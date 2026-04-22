//! Semantic taxonomy over [`HighlightGroup`]s: groups are bucketed into
//! coarser [`CaptureCategory`] families for styling and UI filtering.

use std::fmt;

use crate::highlight::HighlightGroup;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureCategory {
    Annotation,
    Literal,
    Keyword,
    TypeSystem,
    Callable,
    Declaration,
    Value,
    Operator,
    Markup,
    Diagnostic,
}

pub const CATEGORY_COUNT: usize = CaptureCategory::ALL.len();

impl CaptureCategory {
    const ALL: [Self; 10] = [
        Self::Annotation,
        Self::Literal,
        Self::Keyword,
        Self::TypeSystem,
        Self::Callable,
        Self::Declaration,
        Self::Value,
        Self::Operator,
        Self::Markup,
        Self::Diagnostic,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Annotation => "annotation",
            Self::Literal => "literal",
            Self::Keyword => "keyword",
            Self::TypeSystem => "type-system",
            Self::Callable => "callable",
            Self::Declaration => "declaration",
            Self::Value => "value",
            Self::Operator => "operator",
            Self::Markup => "markup",
            Self::Diagnostic => "diagnostic",
        }
    }

    #[must_use]
    pub fn of_group(group: HighlightGroup) -> Self {
        use HighlightGroup::*;
        match group {
            Comment | DocComment => Self::Annotation,
            String | Char | Number | Boolean => Self::Literal,
            Keyword | KeywordControl | KeywordOperator => Self::Keyword,
            Type | TypeBuiltin => Self::TypeSystem,
            Function | Method | Macro => Self::Callable,
            Attribute | Namespace => Self::Declaration,
            Normal | Variable | Parameter | Property | Constant | DiffAdd | DiffRemove => {
                Self::Value
            }
            Operator | Punctuation => Self::Operator,
            Tag | Heading | Emphasis | Link => Self::Markup,
            Error | Warning => Self::Diagnostic,
        }
    }

    #[must_use]
    pub const fn is_literal(self) -> bool {
        matches!(self, Self::Literal)
    }
}

impl fmt::Display for CaptureCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub trait Categorizable {
    fn category(&self) -> CaptureCategory;

    fn belongs_to(&self, target: CaptureCategory) -> bool {
        self.category() == target
    }
}

impl Categorizable for HighlightGroup {
    fn category(&self) -> CaptureCategory {
        CaptureCategory::of_group(*self)
    }
}
