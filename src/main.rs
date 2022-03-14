#![feature(iterator_try_collect, result_option_inspect)]

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

use clap::{IntoApp, Parser};
use console::style;
use dashmap::DashSet;
use indicatif::{MultiProgress, ProgressBar};
use reqwest::{Client, Url};
use synchronoise::CountdownEvent;
use walkdir::WalkDir;
use wmt::{priority_queue::PriorityQueue, progress_style, Settings, Worker};

static APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"),);

/// Recursively download a website
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Target URLs to start from
    #[clap(parse(try_from_str), value_name = "URL")]
    targets: Vec<Url>,

    /// Output path
    #[clap(short, long, parse(from_os_str), default_value = ".")]
    output: PathBuf,

    /// Show progress
    // #[clap(short, long)]
    // progress: bool,

    /// How many threads to use
    #[clap(short, long, default_value_t = num_cpus::get())]
    threads: usize,
}

fn main() {
    let args = Args::parse();

    if args.targets.is_empty() {
        println!("{} no targets provided.\n", style("Error").red());
        Args::command().print_help().unwrap();
    }

    let settings = Settings::builder()
        .output_path(args.output)
        .targets(args.targets)
        .build();

    run_worker_pool(settings, 4);
}

fn run_worker_pool(settings: Settings, threads: usize) {
    let client = Client::builder()
        .user_agent(APP_USER_AGENT)
        .build()
        .unwrap();
    let multi_progress = MultiProgress::new();
    let priority_queue = PriorityQueue::new();
    let checked_urls = DashSet::new();
    let latch = Arc::new(CountdownEvent::new(threads));
    let downloaded_urls = DashSet::new();

    for url in &settings.targets {
        priority_queue.push(url.clone(), None);
        insert_files(&settings.output_path, url, &downloaded_urls);
    }

    (0..threads).for_each(|_| {
        spawn_worker(
            client.clone(),
            priority_queue.clone(),
            &multi_progress,
            settings.clone(),
            checked_urls.clone(),
            downloaded_urls.clone(),
            latch.clone(),
        )
    });

    multi_progress.join().unwrap();
}

fn insert_files(output_path: &Path, url: &Url, urls: &DashSet<Url>) {
    if let Some(host) = url.host_str() {
        WalkDir::new(output_path.join(host))
            .into_iter()
            .filter_map(|e| e.ok())
            .map(|entry| entry.into_path())
            .filter_map(|path| {
                path.strip_prefix(output_path)
                    .map(|path| path.strip_prefix(host).ok())
                    .ok()
                    .flatten()
                    .map(|p| p.display().to_string())
            })
            .filter_map(|path| url.join(&path).ok())
            .for_each(|url| {
                urls.insert(url);
            });
    }
}

fn spawn_worker(
    client: Client,
    priority_queue: PriorityQueue<Url>,
    multi_progress: &MultiProgress,
    settings: Settings,
    checked_urls: DashSet<Url>,
    downloaded_urls: DashSet<Url>,
    latch: Arc<CountdownEvent>,
) {
    let progress_bar = multi_progress
        .add(ProgressBar::new_spinner())
        .with_style(progress_style::spinner())
        .with_message("Starting");

    let worker = Worker::new(
        client,
        priority_queue,
        progress_bar,
        settings,
        checked_urls,
        downloaded_urls,
    );

    thread::spawn(|| worker.run(latch).unwrap());
}

#[cfg(test)]
#[test]
fn verify_app() {
    use clap::CommandFactory;
    Args::command().debug_assert()
}
