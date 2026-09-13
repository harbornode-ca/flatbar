//! Widget state and span definitions.

/// Representation of a styled span in a widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub kind: SpanKind,
    pub emphasis: bool,
}

impl Span {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: SpanKind::Text(text.into()),
            emphasis: false,
        }
    }

    pub fn emphasized_text(text: impl Into<String>) -> Self {
        Self {
            kind: SpanKind::Text(text.into()),
            emphasis: true,
        }
    }

    pub fn icon(icon: impl Into<String>) -> Self {
        Self {
            kind: SpanKind::Icon(icon.into()),
            emphasis: false,
        }
    }

    pub fn emphasized_icon(icon: impl Into<String>) -> Self {
        Self {
            kind: SpanKind::Icon(icon.into()),
            emphasis: true,
        }
    }

    pub fn is_emphasized(&self) -> bool {
        self.emphasis
    }

    pub fn as_text(&self) -> Option<&str> {
        match &self.kind {
            SpanKind::Text(s) => Some(s.as_str()),
            SpanKind::Icon(_) => None,
        }
    }

    pub fn as_icon(&self) -> Option<&str> {
        match &self.kind {
            SpanKind::Icon(s) => Some(s.as_str()),
            SpanKind::Text(_) => None,
        }
    }
}

/// The content type of a span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanKind {
    Text(String),
    Icon(String),
}

/// Immutable snapshot of a widget's current visual state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WidgetState {
    pub spans: Vec<Span>,
    pub tooltip: Option<String>,
}

impl WidgetState {
    pub fn new(spans: Vec<Span>) -> Self {
        Self {
            spans,
            tooltip: None,
        }
    }

    pub fn single_text(text: impl Into<String>) -> Self {
        Self {
            spans: vec![Span::text(text)],
            tooltip: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    pub fn with_tooltip(mut self, tooltip: Option<String>) -> Self {
        self.tooltip = tooltip;
        self
    }
}
