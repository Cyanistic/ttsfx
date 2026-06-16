use crate::config::{ContextExtraction, ContextWindow, PatternFilter};
use crate::{err, Result};
use minijinja::Environment;

const DEFAULT_TEMPLATE: &str = "{{ text }}";

/// Variables available in `sfx_prompt_template`: `text`, `context`, `name`, `tags`.
pub struct RecipeContext<'a> {
    pub context_mode: Option<&'a ContextExtraction>,
    pub full_input: &'a str,
    pub start: usize,
    pub end: usize,
    pub text: &'a str,
    pub filter: &'a PatternFilter,
}

pub fn extract_context_string(
    mode: Option<&ContextExtraction>,
    full_input: &str,
    start: usize,
    end: usize,
) -> String {
    match mode {
        None => String::new(),
        Some(ContextExtraction::Chars(ContextWindow { before, after })) => {
            let b_start = start.saturating_sub(*before);
            let a_end = (end + after).min(full_input.len());
            full_input[b_start..a_end].trim().to_string()
        }
        Some(ContextExtraction::Sentences(window)) => {
            let (sent_start, sent_end) = sentence_bounds(full_input, start, end);
            if full_input[sent_start..sent_end].trim().is_empty() {
                return String::new();
            }
            collect_adjacent_sentences(full_input, sent_start, sent_end, window)
        }
    }
}

fn is_sentence_boundary(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '\n')
}

/// Byte range `[start, end)` of the sentence containing `[match_start, match_end)`.
fn sentence_bounds(full_input: &str, match_start: usize, match_end: usize) -> (usize, usize) {
    let mut sent_start = 0usize;
    for (i, c) in full_input.char_indices() {
        if i >= match_start {
            break;
        }
        if is_sentence_boundary(c) {
            sent_start = i + c.len_utf8();
        }
    }
    let mut sent_end = full_input.len();
    for (i, c) in full_input.char_indices() {
        if i < match_end {
            continue;
        }
        if is_sentence_boundary(c) {
            sent_end = i + c.len_utf8();
            break;
        }
    }
    (sent_start, sent_end)
}

/// Start of the sentence `n` boundaries before `before_pos` (start of containing sentence).
fn prev_sentence_start(full_input: &str, before_pos: usize, n: usize) -> usize {
    let mut starts = vec![0usize];
    for (i, c) in full_input.char_indices() {
        if i < before_pos && is_sentence_boundary(c) {
            starts.push(i + c.len_utf8());
        }
    }
    if n >= starts.len() {
        starts[0]
    } else {
        starts[starts.len() - n - 1]
    }
}

fn next_sentence_end(full_input: &str, after_pos: usize, n: usize) -> usize {
    let mut ends = Vec::new();
    for (i, c) in full_input.char_indices() {
        if i >= after_pos && is_sentence_boundary(c) {
            ends.push(i + c.len_utf8());
        }
    }
    if n == 0 || ends.is_empty() {
        return full_input.len();
    }
    ends.into_iter().nth(n - 1).unwrap_or(full_input.len())
}

fn collect_adjacent_sentences(
    full_input: &str,
    sent_start: usize,
    sent_end: usize,
    ContextWindow { before, after }: &ContextWindow,
) -> String {
    let range_start = if *before == 0 {
        sent_start
    } else {
        prev_sentence_start(full_input, sent_start, *before)
    };
    let range_end = if *after == 0 {
        sent_end
    } else {
        next_sentence_end(full_input, sent_end, *after)
    };
    full_input[range_start..range_end].trim().to_string()
}

pub fn render_recipe(template: Option<&str>, ctx: &RecipeContext<'_>) -> Result<String> {
    let tpl = template.unwrap_or(DEFAULT_TEMPLATE);
    let context = extract_context_string(ctx.context_mode, ctx.full_input, ctx.start, ctx.end);
    let name = ctx.filter.name.as_deref().unwrap_or("");
    let tags = ctx.filter.tags.join(", ");

    let env = Environment::new();
    let compiled = env
        .template_from_str(tpl)
        .map_err(|e| err!(Internal, "recipe template compile failed: {}", e, @external: e))?;
    compiled
        .render(minijinja::context!(
            text => ctx.text,
            context => context,
            name => name,
            tags => tags,
        ))
        .map_err(|e| err!(Internal, "recipe template render failed: {}", e, @external: e))
}
