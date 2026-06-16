use crate::config::PatternFilter;
use tracing::warn;

pub struct PatternMatch {
    pub start: usize,
    pub end: usize,
    pub pattern_index: usize,
}

pub fn match_patterns<'a, I>(patterns: I, text: &str) -> Vec<PatternMatch>
where
    I: Iterator<Item = &'a PatternFilter>,
{
    struct Chunk<'text> {
        offset: usize,
        text: &'text str,
    }

    let mut splits = vec![Chunk { offset: 0, text }];
    let mut matches = Vec::new();
    for (i, pattern) in patterns.enumerate() {
        let mut next = Vec::new();
        for split in &splits {
            let mut matched_here = false;
            for m in pattern.regex.find_iter(split.text) {
                let Ok(m) = m else {
                    warn!(
                        pattern = %pattern.name.as_deref().unwrap_or("?"),
                        "regex error during matching"
                    );
                    continue;
                };
                matched_here = true;
                matches.push(PatternMatch {
                    start: m.start() + split.offset,
                    end: m.end() + split.offset,
                    pattern_index: i,
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
            if !matched_here {
                next.push(Chunk {
                    offset: split.offset,
                    text: split.text,
                });
            }
        }
        splits = next;
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PatternFilter;
    use fancy_regex::Regex;

    fn filter(name: &str, pat: &str) -> PatternFilter {
        PatternFilter {
            name: Some(name.into()),
            priority: 0,
            cache_tag: None,
            tags: vec![],
            regex: Regex::new(pat).unwrap(),
        }
    }

    #[test]
    fn later_pattern_matches_when_earlier_does_not() {
        let patterns = [
            filter("boom", r"\b(?:BOOM|Boom)\b"),
            filter("crash", r"\b(?:CRASH|Crash)\b"),
        ];
        let text = "Listen. CRASH! Done.";
        let matches = match_patterns(patterns.iter(), text);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pattern_index, 1);
        assert_eq!(&text[matches[0].start..matches[0].end], "CRASH");
    }
}
