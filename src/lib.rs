#![feature(try_trait_v2, option_result_contains, result_option_inspect)]

mod escape_path;
pub mod priority_queue;

use std::{
    fs::{create_dir_all, read_to_string, File},
    io::{Error as IoError, Write},
    num::ParseIntError,
    path::{PathBuf, StripPrefixError},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use console::Style;
use dashmap::DashSet;
use indicatif::ProgressBar;
use itertools::Itertools;
use lazy_static::lazy_static;
use reqwest::{
    header::{ToStrError, CONTENT_LENGTH, CONTENT_TYPE},
    Client, Response, Url,
};
use synchronoise::{event::CountdownError, CountdownEvent};
use tokio::{
    runtime::Builder as RuntimeBuilder,
    time::{error::Elapsed, timeout},
};
use typed_builder::TypedBuilder;

use crate::{
    escape_path::EscapePathExt,
    priority_queue::{Priority, PriorityQueue},
};

lazy_static! {
    pub(crate) static ref STATUS_WORKING_STYLE: Style = Style::new().cyan().bold();
    pub(crate) static ref STATUS_OK_STYLE: Style = Style::new().green().bold();
    pub(crate) static ref STATUS_WARN_STYLE: Style = Style::new().yellow().bold();
    pub(crate) static ref STATUS_ERROR_STYLE: Style = Style::new().red().bold();
}

pub mod progress_style {
    use indicatif::ProgressStyle;

    const FIRA_CODE_TICK_CHARS: &str = "\u{EE06}\u{EE07}\u{EE08}\u{EE09}\u{EE0A}\u{EE0B}";

    pub fn spinner() -> ProgressStyle {
        ProgressStyle::default_spinner()
            .template("{spinner} {prefix:>11.cyan.bold} {wide_msg}\n")
            .tick_chars(FIRA_CODE_TICK_CHARS)
    }

    pub fn bar() -> ProgressStyle {
        ProgressStyle::default_bar().template(
            "{prefix:>13.cyan.bold} {wide_msg}\n{bytes_per_sec:>13} {bytes:>9}/{total_bytes:>9} [{wide_bar}]",
        ).progress_chars("=> ")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Failed to parse document")]
    HtmlParse(
        #[source]
        #[from]
        tl::ParseError,
    ),

    #[error("Failed to send reqwest")]
    SendRequest(#[source] reqwest::Error),

    #[error("Failed to get response body")]
    GetResponseBody(#[source] reqwest::Error),

    #[error("Failed to strip path")]
    StripPath(
        #[source]
        #[from]
        StripPrefixError,
    ),

    #[error("Failed to create file")]
    CreateFile(#[source] IoError),

    #[error("Failed to write to file")]
    WriteFile(#[source] IoError),

    #[error("Failed to read file to string")]
    ReadFile(#[source] IoError),

    #[error("Failed to build tokio runtime")]
    BuildRuntime(#[source] IoError),

    #[error("Failed to convert header value to string")]
    HeaderValueToString(
        #[source]
        #[from]
        ToStrError,
    ),

    #[error("Failed to parse content length (`{value}`)")]
    ParseContentLength {
        #[source]
        err: ParseIntError,
        value: String,
    },

    #[error("Connection timed out")]
    TimedOut(Elapsed),

    #[error("Failed to decrement latch: {0:?}")]
    DecrementLatch(CountdownError),

    #[error("Failed to increment latch: {0:?}")]
    IncrementLatch(CountdownError),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, TypedBuilder)]
pub struct Settings {
    /// The output path
    #[builder(setter(into))]
    pub output_path: PathBuf,

    pub targets: Vec<Url>,
}

#[derive(Debug, Clone)]
pub struct Worker {
    /// Worker Settings
    settings: Settings,
    /// reqwest HTTP Client
    client: Client,
    /// Progress Bar
    progress_bar: ProgressBar,
    /// Job queue with priority
    priority_queue: PriorityQueue<Url>,
    /// List of already checked urls
    checked_urls: DashSet<Url>,
    /// List of previously downloaded files
    downloaded_urls: DashSet<Url>,
}

impl Worker {
    pub fn new(
        client: Client,
        priority_queue: PriorityQueue<Url>,
        progress_bar: ProgressBar,
        settings: Settings,
        checked_urls: DashSet<Url>,
        downloaded_urls: DashSet<Url>,
    ) -> Self {
        progress_bar.enable_steady_tick(100);
        Self {
            client,
            progress_bar,
            priority_queue,
            settings,
            checked_urls,
            downloaded_urls,
        }
    }

    pub fn run(self, latch: Arc<CountdownEvent>) -> Result<()> {
        let runtime = RuntimeBuilder::new_current_thread()
            .enable_all()
            .build()
            .map_err(Error::BuildRuntime)?;

        runtime.block_on(self._run(&latch))
    }

    // TODO: prevent urls from beeing checked twice
    async fn _run(&self, latch: &CountdownEvent) -> Result<()> {
        self.progress_bar.set_prefix("Idle");

        loop {
            if let Some(url) = self.priority_queue.pop() {
                if self.checked_urls.contains(&url) {
                    continue;
                }

                self.progress_bar.set_message(url.to_string());

                if let Err(err) = self.work(&url).await {
                    self.progress_bar.println(format!(
                        "{} while downloading {url}: {err}",
                        STATUS_ERROR_STYLE.apply_to("Error"),
                    ));

                    self.reset_progress_bar();

                    // requeue job
                    self.priority_queue.push(url, Priority::Normal)
                }

                self.progress_bar.set_prefix("Idle");
                self.progress_bar.set_message("");
            } else {
                self.progress_bar.set_prefix("Idle");
                // decrement busy workers by one
                latch.decrement().map_err(Error::DecrementLatch)?;

                // park_with_timeout
                latch.wait_timeout(Duration::from_secs(1));

                // if number of busy workers is zero and queue is empty then leave
                if latch.count() == 0 && self.priority_queue.is_empty() {
                    break;
                }

                // else repeat and increment workers by one
                latch.increment().map_err(Error::IncrementLatch)?;
            }
        }

        self.progress_bar.finish_using_style();

        Ok(())
    }

    async fn work(&self, url: &Url) -> Result<()> {
        self.download(url.clone()).await?;

        self.progress_bar
            .println(format!("{:>13} {url}", STATUS_OK_STYLE.apply_to("Saved"),));

        if !self.checked_urls.insert(url.clone()) {
            // warn url was checked twice
            self.progress_bar.println(format!(
                "{}: Checked {url} twice",
                STATUS_WARN_STYLE.apply_to("Warning"),
            ))
        };

        Ok(())
    }

    async fn download(&self, url: Url) -> Result<()> {
        self.progress_bar.set_prefix("Downloading");
        let mut res = self
            .client
            .get(url)
            .send()
            .await
            .map_err(Error::SendRequest)?;

        let content_length = res
            .headers()
            .get(CONTENT_LENGTH)
            .map(|header_value| header_value.to_str())
            .transpose()?
            .map(|src| {
                u64::from_str(src).map_err(|err| Error::ParseContentLength {
                    err,
                    value: src.to_string(),
                })
            })
            .transpose()?;

        let path = self.save_response_to_disk(&mut res, content_length).await?;

        let is_html = res
            .headers()
            .get(CONTENT_TYPE)
            .map(|value| value.to_str())
            .transpose()?
            .map(|s| s == "text/html")
            .unwrap_or_default();

        if is_html {
            let document = read_to_string(path).map_err(Error::ReadFile)?;
            self.parse(res.url(), &document)?;
        }

        Ok(())
    }

    async fn save_response_to_disk(
        &self,
        response: &mut Response,
        content_length: Option<u64>,
    ) -> Result<PathBuf> {
        let path = url_to_path(response.url()).unwrap();
        let mut output_path = self.settings.output_path.join(path);

        if let Some(parent) = output_path.parent() {
            if !parent.exists() {
                create_dir_all(parent).unwrap();
            }
        }

        if output_path.is_dir() {
            output_path = output_path.join("index.html")
        }

        let file = File::create(&output_path).map_err(Error::CreateFile)?;

        if let Some(content_length) = content_length {
            self.progress_bar.set_length(content_length);
            self.progress_bar.set_style(progress_style::bar());

            Self::save_to_disk(response, self.progress_bar.wrap_write(file)).await?;

            self.reset_progress_bar();
        } else {
            Self::save_to_disk(response, file).await?;
        }

        Ok(output_path)
    }

    fn reset_progress_bar(&self) {
        self.progress_bar.set_length(0);
        self.progress_bar.set_style(progress_style::spinner());
    }

    async fn save_to_disk<Writer>(response: &mut Response, mut writer: Writer) -> Result<()>
    where
        Writer: Write,
    {
        while let Some(chunk) = timeout(Duration::from_secs(3), response.chunk())
            .await
            .map_err(Error::TimedOut)?
            .map_err(Error::GetResponseBody)?
        {
            writer.write_all(&chunk).map_err(Error::WriteFile)?;
        }

        Ok(())
    }

    fn parse(&self, base_url: &Url, document: &str) -> Result<()> {
        let dom = tl::parse(document, tl::ParserOptions::default())?;

        // get urls
        dom.query_selector("a[href]")
            .unwrap()
            .filter_map(|handle| handle.get(dom.parser()))
            .filter_map(|node| node.as_tag())
            .filter_map(|tag| tag.attributes().get("href").flatten())
            .map(|bytes| bytes.as_utf8_str())
            // filter out relative urls to parent urls
            .filter(|s| !s.starts_with(".."))
            .filter_map(|s| match Url::parse(&s) {
                Err(<Url as FromStr>::Err::RelativeUrlWithoutBase) => base_url
                    .join(&s)
                    .inspect_err(|err| {
                        self.progress_bar.println(format!(
                            "{} parsing relative URL `{s}`: {err:?}",
                            STATUS_ERROR_STYLE.apply_to("Error"),
                        ));
                    })
                    .ok(),
                Err(err) => {
                    self.progress_bar.println(format!(
                        "{} parsing URL `{s}`: {err:?}",
                        STATUS_ERROR_STYLE.apply_to("Error"),
                    ));
                    None
                }
                Ok(url) => Some(url),
            })
            // check urls
            .filter(|url| !self.checked_urls.contains(url))
            .cartesian_product(self.settings.targets.iter())
            .filter(|(url, target)| url.domain() == target.domain())
            .filter(|(url, target)| url.path().starts_with(target.path()))
            .for_each(|(url, _)| {
                let priority = if self.downloaded_urls.contains(&url) {
                    Priority::Low
                } else {
                    Priority::Normal
                };
                self.priority_queue.push(url.clone(), priority)
            });

        Ok(())
    }
}

fn url_to_path(url: &Url) -> Option<PathBuf> {
    if url.cannot_be_a_base() {
        return None;
    }

    let domain = url.domain()?;
    let base = format!("{domain}{}", url.path());
    let file_name = merge_file_name_and_query(url)?;

    match base.rsplit_once('/') {
        Some((_, "")) => Some(PathBuf::from(format!("{base}{file_name}"))),
        Some((_, _)) => Some(PathBuf::from(base).with_file_name(file_name)),
        _ => None,
    }
}

fn merge_file_name_and_query(url: &Url) -> Option<String> {
    let file_name = match url.path_segments()?.last()? {
        "" => "index.html",
        file_name => file_name,
    };

    let file_name = if let Some(query) = url.query() {
        format!("{file_name}?{}", query.escape_path())
    } else {
        file_name.to_string()
    };

    Some(file_name)
}

#[cfg(test)]
mod test {
    pub use super::*;

    mod merge_file_name_and_query {
        use reqwest::Url;

        use super::*;

        #[test]
        fn with_trailing_slash() {
            let url = Url::parse("https://www.google.com/").unwrap();

            assert_eq!(
                Some(String::from("index.html")),
                merge_file_name_and_query(&url)
            )
        }

        #[test]
        fn with_out_trailing_slash() {
            let url = Url::parse("https://google.com").unwrap();

            assert_eq!(
                Some(String::from("index.html")),
                merge_file_name_and_query(&url)
            )
        }

        #[test]
        fn with_query() {
            let url = Url::parse("http://video.google.de/?hl=de&tab=wv").unwrap();

            assert_eq!(
                Some(String::from("index.html?hl=de&tab=wv")),
                merge_file_name_and_query(&url)
            )
        }

        #[test]
        fn with_file() {
            let url = Url::parse("http://www.google.de/index.html").unwrap();

            assert_eq!(
                Some(String::from("index.html")),
                merge_file_name_and_query(&url)
            )
        }
    }

    mod url_to_path {
        use std::ffi::OsString;

        use reqwest::Url;

        use super::*;

        #[test]
        fn google_homepage() {
            let url = Url::parse("https://www.google.com/").unwrap();

            assert_eq!(
                Some(PathBuf::from("www.google.com/index.html")),
                url_to_path(&url)
            );
        }

        #[test]
        fn with_parameters() {
            let url = Url::parse("http://video.google.de/?hl=de&tab=wv").unwrap();

            assert_eq!(
                Some(PathBuf::from("video.google.de/index.html?hl=de&tab=wv")),
                url_to_path(&url)
            );
        }

        #[test]
        fn with_file() {
            let url = Url::parse("http://video.google.de/some_page").unwrap();

            assert_eq!(
                Some(PathBuf::from("video.google.de/some_page")),
                url_to_path(&url)
            );
        }

        #[test]
        fn url_in_query() {
            let url = Url::parse("https://accounts.google.com/ServiceLogin?hl=de&passive=true&continue=https://www.google.com/&ec=GAZAAQ").unwrap();

            let path = url_to_path(&url).unwrap();

            assert_eq!(
                PathBuf::from("accounts.google.com/ServiceLogin?hl=de&passive=true&continue=https:\u{2215}\u{2215}www.google.com\u{2215}&ec=GAZAAQ"),
                path
            );

            let osstring = OsString::from(
                "ServiceLogin?hl=de&passive=true&continue=https:\u{2215}\u{2215}www.google.com\u{2215}&ec=GAZAAQ",
            );
            assert_eq!(
                Some(osstring.as_os_str()),
                path.file_name(),
                "file name should be last url segment including query"
            )
        }
    }
}
