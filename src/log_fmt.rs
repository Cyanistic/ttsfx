//! CLI event formatter: message and fields first, dimmed active-span trail.

use std::fmt;

use nu_ansi_term::{Color, Style};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::{
    fmt::{
        format::{self, FormatEvent, FormatFields},
        time::{FormatTime, SystemTime},
        FmtContext, FormattedFields,
    },
    registry::LookupSpan,
};

use crate::span_transform::SpanDisplayHint;

pub struct SpanTreeFormat;

fn level_style(level: &Level) -> Style {
    match *level {
        Level::ERROR => Color::Red.bold(),
        Level::WARN => Color::Yellow.bold(),
        Level::INFO => Color::Green.bold(),
        Level::DEBUG => Color::Blue.bold(),
        Level::TRACE => Color::Purple.bold(),
    }
}

fn write_dim(writer: &mut format::Writer<'_>, dim: Style, text: &str) -> fmt::Result {
    if writer.has_ansi_escapes() {
        write!(writer, "{}", dim.paint(text))
    } else {
        write!(writer, "{text}")
    }
}

fn write_separator(writer: &mut format::Writer<'_>, dim: Style) -> fmt::Result {
    write_dim(writer, dim, " > ")
}

impl<S, N> FormatEvent<S, N> for SpanTreeFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: format::Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let metadata = event.metadata();
        let level = metadata.level();
        let ansi = writer.has_ansi_escapes();

        SystemTime.format_time(&mut writer)?;
        write!(writer, " ")?;

        if ansi {
            write!(writer, "{:>5} ", level_style(level).paint(level.as_str()))?;
        } else {
            write!(writer, "{level:>5} ")?;
        }

        ctx.field_format().format_fields(writer.by_ref(), event)?;

        let dim = if ansi {
            Style::new().dimmed()
        } else {
            Style::new()
        };

        let mut wrote_span = false;

        if let Some(scope) = ctx.event_scope() {
            for span in scope.from_root() {
                let ext = span.extensions();
                let hint = ext.get::<SpanDisplayHint>();

                if hint.is_some_and(|h| h.hidden) {
                    continue;
                }

                if wrote_span {
                    write_separator(&mut writer, dim)?;
                } else {
                    write!(writer, "  ")?;
                }
                wrote_span = true;

                if let Some(display_name) = hint.and_then(|h| h.display_name.as_deref()) {
                    write_dim(&mut writer, dim, display_name)?;
                } else {
                    let name = span.name();
                    let fields = ext.get::<FormattedFields<N>>();
                    let has_fields = fields.as_ref().is_some_and(|f| !f.is_empty());

                    if has_fields {
                        write_dim(&mut writer, dim, &format!("{name}{{{}}}", fields.unwrap()))?;
                    } else {
                        write_dim(&mut writer, dim, name)?;
                    }
                }
            }
        }

        write_dim(&mut writer, dim, &format!("  {}", metadata.target()))?;
        writeln!(writer)
    }
}
