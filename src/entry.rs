//! Parsing of `cliphist list` output.

/// One line of `cliphist list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The line exactly as cliphist printed it. `cliphist decode` and
    /// `cliphist delete` take this on stdin, so it is never rebuilt.
    pub raw: String,
    pub id: u64,
    pub preview: String,
    pub kind: Kind,
    /// What the list shows and what search matches against. The same as
    /// `preview`, except images get a readable description.
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// A single http(s) URL and nothing else.
    Url,
    Image {
        format: String,
        width: u32,
        height: u32,
        size: String,
    },
}

impl Entry {
    /// Parses one line (`<id>\t<preview>`). Returns `None` for malformed lines.
    pub fn parse(line: &str) -> Option<Self> {
        let (id, preview) = line.split_once('\t')?;
        let id = id.parse().ok()?;
        let kind = parse_image(preview).unwrap_or_else(|| {
            if is_url(preview) {
                Kind::Url
            } else {
                Kind::Text
            }
        });
        let label = match &kind {
            Kind::Image {
                format,
                width,
                height,
                size,
            } => format!("[image] {format} {width}x{height} · {size}"),
            Kind::Text | Kind::Url => preview.to_owned(),
        };
        Some(Self {
            raw: line.to_owned(),
            id,
            preview: preview.to_owned(),
            kind,
            label,
        })
    }
}

/// Parses cliphist's image placeholder, e.g. `[[ binary data 2 MiB png 2560x1440 ]]`.
fn parse_image(preview: &str) -> Option<Kind> {
    let inner = preview
        .strip_prefix("[[ binary data ")?
        .strip_suffix(" ]]")?;
    // "<num> <unit> <format> <w>x<h>"
    let mut parts = inner.split(' ');
    let num = parts.next()?;
    let unit = parts.next()?;
    let format = parts.next()?;
    let (w, h) = parts.next()?.split_once('x')?;
    if parts.next().is_some() {
        return None;
    }
    Some(Kind::Image {
        format: format.to_owned(),
        width: w.parse().ok()?,
        height: h.parse().ok()?,
        size: format!("{num} {unit}"),
    })
}

/// True if the preview is one http(s) URL with nothing around it.
fn is_url(preview: &str) -> bool {
    let p = preview.trim();
    let rest = p
        .strip_prefix("https://")
        .or_else(|| p.strip_prefix("http://"));
    matches!(rest, Some(r) if !r.is_empty() && !r.contains(char::is_whitespace))
}

/// Parses the full output of `cliphist list`, skipping malformed lines.
pub fn parse_list(output: &str) -> Vec<Entry> {
    output.lines().filter_map(Entry::parse).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_entry() {
        let e = Entry::parse("42\thello\tworld").unwrap();
        assert_eq!(e.id, 42);
        assert_eq!(e.preview, "hello\tworld");
        assert_eq!(e.kind, Kind::Text);
        assert_eq!(e.raw, "42\thello\tworld");
    }

    #[test]
    fn parses_image_entry() {
        let e = Entry::parse("4610\t[[ binary data 2 MiB png 2560x1440 ]]").unwrap();
        assert_eq!(
            e.kind,
            Kind::Image {
                format: "png".into(),
                width: 2560,
                height: 1440,
                size: "2 MiB".into(),
            }
        );
        assert_eq!(e.label, "[image] png 2560x1440 · 2 MiB");
    }

    #[test]
    fn text_that_looks_like_binary_is_text() {
        let e = Entry::parse("1\t[[ binary data is cool ]]").unwrap();
        assert_eq!(e.kind, Kind::Text);
    }

    #[test]
    fn detects_urls() {
        let kind = |p: &str| Entry::parse(&format!("1\t{p}")).unwrap().kind;
        assert_eq!(kind("https://example.com/a?b=c"), Kind::Url);
        assert_eq!(kind("  http://example.com  "), Kind::Url);
        assert_eq!(kind("https://"), Kind::Text);
        assert_eq!(kind("see https://example.com"), Kind::Text);
        assert_eq!(kind("https://a.com https://b.com"), Kind::Text);
        assert_eq!(kind("magnet:?xt=urn:btih:abc"), Kind::Text);
    }

    #[test]
    fn skips_malformed_lines() {
        let list = parse_list("1\ta\nnot-a-line\nx\tb\n2\tc\n");
        let ids: Vec<_> = list.iter().map(|e| e.id).collect();
        assert_eq!(ids, [1, 2]);
    }
}
