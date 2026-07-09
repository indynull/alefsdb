//! Absolute database paths shared by API, query, and FUSE.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DbPath {
    /// Path segments after the root. Empty means `/`.
    segments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    Empty,
    NotAbsolute,
    EmptySegment,
    ForbiddenSegment(String),
    InvalidUtf8,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathError::Empty => write!(f, "empty path"),
            PathError::NotAbsolute => write!(f, "path must be absolute (start with /)"),
            PathError::EmptySegment => write!(f, "path contains an empty segment"),
            PathError::ForbiddenSegment(s) => {
                write!(f, "path segment '{s}' is forbidden (. and .. are not stored)")
            }
            PathError::InvalidUtf8 => write!(f, "path is not valid UTF-8"),
        }
    }
}

impl std::error::Error for PathError {}

impl DbPath {
    /// Parse and validate an absolute path string.
    pub fn parse(input: &str) -> Result<Self, PathError> {
        if input.is_empty() {
            return Err(PathError::Empty);
        }
        if !input.starts_with('/') {
            return Err(PathError::NotAbsolute);
        }
        if input == "/" {
            return Ok(DbPath {
                segments: Vec::new(),
            });
        }
        // Reject trailing slash for non-root (keeps canonical string form unique).
        let trimmed = input.trim_end_matches('/');
        if trimmed.is_empty() {
            return Ok(DbPath {
                segments: Vec::new(),
            });
        }
        let mut segments = Vec::new();
        for seg in trimmed[1..].split('/') {
            if seg.is_empty() {
                return Err(PathError::EmptySegment);
            }
            if seg == "." || seg == ".." {
                return Err(PathError::ForbiddenSegment(seg.to_owned()));
            }
            if seg.as_bytes().contains(&0) {
                return Err(PathError::InvalidUtf8);
            }
            segments.push(seg.to_owned());
        }
        Ok(DbPath { segments })
    }

    pub fn root() -> Self {
        DbPath {
            segments: Vec::new(),
        }
    }

    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    pub fn join(&self, segment: &str) -> Result<Self, PathError> {
        if segment.is_empty() {
            return Err(PathError::EmptySegment);
        }
        if segment == "." || segment == ".." || segment.contains('/') {
            return Err(PathError::ForbiddenSegment(segment.to_owned()));
        }
        let mut segments = self.segments.clone();
        segments.push(segment.to_owned());
        Ok(DbPath { segments })
    }

    pub fn parent(&self) -> Option<Self> {
        if self.segments.is_empty() {
            return None;
        }
        let mut segments = self.segments.clone();
        segments.pop();
        Some(DbPath { segments })
    }

    /// Canonical string form (`/` or `/a/b`).
    pub fn as_str(&self) -> String {
        if self.segments.is_empty() {
            return "/".to_owned();
        }
        let mut s = String::from("/");
        s.push_str(&self.segments.join("/"));
        s
    }
}

impl std::fmt::Display for DbPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_root_and_nested() {
        assert_eq!(DbPath::parse("/").unwrap().as_str(), "/");
        assert!(DbPath::parse("/").unwrap().is_root());
        let p = DbPath::parse("/users/alice").unwrap();
        assert_eq!(p.as_str(), "/users/alice");
        assert_eq!(p.segments(), &["users".to_string(), "alice".to_string()]);
    }

    #[test]
    fn trims_trailing_slash() {
        assert_eq!(DbPath::parse("/a/b/").unwrap().as_str(), "/a/b");
    }

    #[test]
    fn rejects_relative_and_empty() {
        assert_eq!(DbPath::parse(""), Err(PathError::Empty));
        assert_eq!(DbPath::parse("a/b"), Err(PathError::NotAbsolute));
    }

    #[test]
    fn rejects_empty_and_dot_segments() {
        assert_eq!(DbPath::parse("/a//b"), Err(PathError::EmptySegment));
        assert!(matches!(
            DbPath::parse("/./x"),
            Err(PathError::ForbiddenSegment(_))
        ));
        assert!(matches!(
            DbPath::parse("/a/../b"),
            Err(PathError::ForbiddenSegment(_))
        ));
    }

    #[test]
    fn join_and_parent() {
        let p = DbPath::parse("/a").unwrap();
        let child = p.join("b").unwrap();
        assert_eq!(child.as_str(), "/a/b");
        assert_eq!(child.parent().unwrap().as_str(), "/a");
        assert_eq!(DbPath::root().parent(), None);
    }

    #[test]
    fn join_rejects_slashes_and_dots() {
        let p = DbPath::root();
        assert!(p.join("a/b").is_err());
        assert!(p.join("..").is_err());
        assert!(p.join("").is_err());
    }
}
