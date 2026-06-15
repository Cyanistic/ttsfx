use crate::config::{ContextExtraction, PatternFilter};
use crate::{Result, err};
use minijinja::Environment;

const DEFAULT_TEMPLATE: &str = "{{ text }}";

/// Variables available in `sfx_prompt_template`: `text`, `before`, `after`, `sentence`, `name`, `tags`.
pub struct RecipeContext<'a> {
    pub context_mode: &'a ContextExtraction,
    pub full_input: &'a str,
    pub start: usize,
    pub end: usize,
    pub text: &'a str,
    pub filter: &'a PatternFilter,
}

pub fn extract_context(
    mode: &ContextExtraction,
    full_input: &str,
    start: usize,
    end: usize,
) -> (String, String, Option<String>) {
    match mode {
        ContextExtraction::None => (String::new(), String::new(), None),
        ContextExtraction::Chars {
            before: b,
            after: a,
        } => {
            let b_start = start.saturating_sub(*b);
            let a_end = (end + a).min(full_input.len());
            (
                full_input[b_start..start].to_string(),
                full_input[end..a_end].to_string(),
                None,
            )
        }
        ContextExtraction::Sentence => {
            let sentence = extract_sentence(full_input, start, end);
            (String::new(), String::new(), sentence)
        }
    }
}

fn extract_sentence(full_input: &str, start: usize, end: usize) -> Option<String> {
    let boundary = |c: char| matches!(c, '.' | '!' | '?' | '\n');
    let mut sent_start = 0usize;
    for (i, c) in full_input.char_indices() {
        if i >= start {
            break;
        }
        if boundary(c) {
            sent_start = i + c.len_utf8();
        }
    }
    let mut sent_end = full_input.len();
    for (i, c) in full_input.char_indices() {
        if i < end {
            continue;
        }
        if boundary(c) {
            sent_end = i + c.len_utf8();
            break;
        }
    }
    let slice = full_input[sent_start..sent_end].trim();
    if slice.is_empty() {
        None
    } else {
        Some(slice.to_string())
    }
}

pub fn render_recipe(template: Option<&str>, ctx: &RecipeContext<'_>) -> Result<String> {
    let tpl = template.unwrap_or(DEFAULT_TEMPLATE);
    let (before, after, sentence) =
        extract_context(ctx.context_mode, ctx.full_input, ctx.start, ctx.end);
    let name = ctx.filter.name.as_deref().unwrap_or("");
    let tags = ctx.filter.tags.join(", ");
    let sentence_str = sentence.as_deref().unwrap_or("");

    let env = Environment::new();
    let compiled = env
        .template_from_str(tpl)
        .map_err(|e| err!(Internal, "recipe template compile failed: {}", e, @external: e))?;
    compiled
        .render(minijinja::context!(
            text => ctx.text,
            before => before,
            after => after,
            sentence => sentence_str,
            name => name,
            tags => tags,
        ))
        .map_err(|e| err!(Internal, "recipe template render failed: {}", e, @external: e))
}
