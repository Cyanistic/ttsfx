use crate::config::PatternFilter;
use tracing::warn;

pub struct PatternMatch<'filter> {
    pub start: usize,
    pub end: usize,
    pub filter: &'filter PatternFilter,
}

pub fn match_patterns<'filter>(
    patterns: &'filter [PatternFilter],
    text: &str,
) -> Vec<PatternMatch<'filter>> {
    struct Chunk<'text> {
        offset: usize,
        text: &'text str,
    }

    let mut splits = vec![Chunk { offset: 0, text }];
    let mut matches = Vec::new();
    for pattern in patterns {
        let mut next = Vec::new();
        for split in &splits {
            for m in pattern.regex.find_iter(split.text) {
                let Ok(m) = m else {
                    warn!(
                        pattern = %pattern.name.as_deref().unwrap_or("?"),
                        "regex error during matching"
                    );
                    continue;
                };
                matches.push(PatternMatch {
                    start: m.start() + split.offset,
                    end: m.end() + split.offset,
                    filter: pattern,
                });
                next.push(Chunk {
                    offset: split.offset,
                    text: &split.text[..m.start()],
                });
                next.push(Chunk {
                    offset: split.offset + m.end(),
                    text: &split.text[m.end()..],
                });
            }
        }
        splits = next;
    }
    matches
}
