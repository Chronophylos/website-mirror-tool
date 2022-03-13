use std::{
    fmt::{self, Write},
    iter::{FlatMap, FusedIterator},
    ops::Try,
    str::Chars,
};

use self::char::CharExt;

pub trait EscapePathExt {
    fn escape_path(&self) -> EscapePath;
}

impl EscapePathExt for String {
    fn escape_path(&self) -> EscapePath {
        EscapePath {
            inner: self.chars().flat_map(escape_path),
        }
    }
}

impl EscapePathExt for str {
    fn escape_path(&self) -> EscapePath {
        EscapePath {
            inner: self.chars().flat_map(escape_path),
        }
    }
}

impl<T> EscapePathExt for &T
where
    T: EscapePathExt,
{
    fn escape_path(&self) -> EscapePath {
        (*self).escape_path()
    }
}

#[derive(Debug, Clone)]
pub struct EscapePath<'a> {
    inner: FlatMap<Chars<'a>, char::EscapePath, CharEscapePath>,
}

impl<'a> fmt::Display for EscapePath<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.clone().try_for_each(|c| f.write_char(c))
    }
}

impl<'a> Iterator for EscapePath<'a> {
    type Item = char;

    #[inline]
    fn next(&mut self) -> Option<char> {
        self.inner.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }

    #[inline]
    fn try_fold<Acc, Fold, R>(&mut self, init: Acc, fold: Fold) -> R
    where
        Self: Sized,
        Fold: FnMut(Acc, Self::Item) -> R,
        R: Try<Output = Acc>,
    {
        self.inner.try_fold(init, fold)
    }

    #[inline]
    fn fold<Acc, Fold>(self, init: Acc, fold: Fold) -> Acc
    where
        Fold: FnMut(Acc, Self::Item) -> Acc,
    {
        self.inner.fold(init, fold)
    }
}

impl<'a> FusedIterator for EscapePath<'a> {}

type CharEscapePath = fn(char) -> char::EscapePath;

fn escape_path(c: char) -> char::EscapePath {
    c.escape_path()
}

mod char {
    use std::{
        char::EscapeDefault,
        fmt::{self, Write},
        iter::FusedIterator,
    };

    #[derive(Debug, Clone)]
    pub struct EscapePath {
        state: EscapePathState,
    }

    #[derive(Debug, Clone)]
    enum EscapePathState {
        Done,
        Char(char),
        //Backslash(char),
        Default(EscapeDefault),
    }

    impl Iterator for EscapePath {
        type Item = char;

        fn next(&mut self) -> Option<Self::Item> {
            match self.state {
                EscapePathState::Done => None,
                //EscapePathState::Backslash(c) => {
                //    self.state = EscapePathState::Char(c);
                //    Some('\\')
                //}
                EscapePathState::Char(c) => {
                    self.state = EscapePathState::Done;
                    Some(c)
                }
                EscapePathState::Default(ref mut iter) => iter.next(),
            }
        }

        #[inline]
        fn size_hint(&self) -> (usize, Option<usize>) {
            let n = self.len();
            (n, Some(n))
        }

        #[inline]
        fn count(self) -> usize {
            self.len()
        }

        fn nth(&mut self, n: usize) -> Option<char> {
            match self.state {
                //EscapePathState::Backslash(c) if n == 0 => {
                //    self.state = EscapePathState::Char(c);
                //    Some('\\')
                //}
                //EscapePathState::Backslash(c) if n == 1 => {
                //    self.state = EscapePathState::Done;
                //    Some(c)
                //}
                //EscapePathState::Backslash(_) => {
                //    self.state = EscapePathState::Done;
                //    None
                //}
                EscapePathState::Char(c) => {
                    self.state = EscapePathState::Done;

                    if n == 0 {
                        Some(c)
                    } else {
                        None
                    }
                }
                EscapePathState::Done => None,
                EscapePathState::Default(ref mut i) => i.nth(n),
            }
        }

        fn last(self) -> Option<char> {
            match self.state {
                EscapePathState::Default(iter) => iter.last(),
                EscapePathState::Done => None,
                /*EscapePathState::Backslash(c) |*/ EscapePathState::Char(c) => Some(c),
            }
        }
    }

    impl ExactSizeIterator for EscapePath {
        fn len(&self) -> usize {
            match self.state {
                EscapePathState::Done => 0,
                EscapePathState::Char(_) => 1,
                //EscapePathState::Backslash(_) => 2,
                EscapePathState::Default(ref iter) => iter.len(),
            }
        }
    }

    impl FusedIterator for EscapePath {}

    impl fmt::Display for EscapePath {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for c in self.clone() {
                f.write_char(c)?;
            }
            Ok(())
        }
    }

    pub(super) trait CharExt {
        fn escape_path(self) -> EscapePath;
    }

    impl CharExt for char {
        fn escape_path(self) -> EscapePath {
            let state = match self {
                '/' => EscapePathState::Char('\u{2215}'),
                _ => EscapePathState::Default(self.escape_default()),
            };
            EscapePath { state }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::escape_path::EscapePathExt;

    #[test]
    fn escape_path_url() {
        assert_eq!(
            "https:\u{2215}\u{2215}www.google.com\u{2215}&ec=GAZAAQ".to_string(),
            "https://www.google.com/&ec=GAZAAQ"
                .escape_path()
                .collect::<String>()
        );
    }
}
