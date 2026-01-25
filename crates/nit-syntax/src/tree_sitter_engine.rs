use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use nit_core::{BufferEdit, BufferPoint};
use tree_sitter::{InputEdit, Parser, Point, Tree};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};
use tracing::{error, warn};

use crate::engine::{HighlightRequest, SyntaxEngine};
use crate::highlight::{EngineKind, HighlightGroup, HighlightSnapshot, HighlightSpan, SyntaxStatus};
use crate::registry::{LanguageId, LanguageRegistry};

pub struct TreeSitterEngine {
    sender: Sender<HighlightRequest>,
    receiver: Receiver<HighlightResult>,
    cache: HashMap<usize, HighlightSnapshot>,
}

struct HighlightResult {
    buffer_id: usize,
    snapshot: HighlightSnapshot,
}

impl TreeSitterEngine {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<HighlightRequest>();
        let (out_tx, out_rx) = mpsc::channel::<HighlightResult>();
        thread::spawn(move || worker_loop(rx, out_tx));
        Self {
            sender: tx,
            receiver: out_rx,
            cache: HashMap::new(),
        }
    }

    fn drain_results(&mut self) {
        while let Ok(result) = self.receiver.try_recv() {
            self.cache.insert(result.buffer_id, result.snapshot);
        }
    }
}

impl SyntaxEngine for TreeSitterEngine {
    fn detect_language(
        &self,
        path: Option<&std::path::Path>,
        first_line: Option<&str>,
        override_lang: Option<LanguageId>,
    ) -> LanguageId {
        LanguageRegistry::detect(path, first_line, override_lang)
    }

    fn schedule_rehighlight(&mut self, request: HighlightRequest) {
        let _ = self.sender.send(request);
    }

    fn try_get_highlights(&mut self, buffer_id: usize, version: u64) -> Option<HighlightSnapshot> {
        self.drain_results();
        self.cache
            .get(&buffer_id)
            .filter(|snap| snap.version == version)
            .cloned()
    }
}

struct BufferState {
    language: LanguageId,
    parser: Parser,
    tree: Option<Tree>,
}

fn worker_loop(rx: Receiver<HighlightRequest>, out_tx: Sender<HighlightResult>) {
    let mut buffers: HashMap<usize, BufferState> = HashMap::new();
    let mut configs = build_configs();
    let mut highlighter = Highlighter::new();

    loop {
        let first = match rx.recv() {
            Ok(job) => job,
            Err(_) => break,
        };
        let mut pending: HashMap<usize, HighlightRequest> = HashMap::new();
        pending.insert(first.buffer_id, first);
        while let Ok(job) = rx.try_recv() {
            pending.insert(job.buffer_id, job);
        }

        for (_, job) in pending.drain() {
            let snapshot = match highlight_job(&mut buffers, &mut configs, &mut highlighter, &job) {
                Ok(snap) => snap,
                Err(err) => {
                    error!("syntax highlight error: {err}");
                    HighlightSnapshot::plain(
                        job.buffer_id,
                        job.version,
                        job.language,
                        EngineKind::TreeSitter,
                        SyntaxStatus::Error(err.to_string()),
                        &job.text,
                    )
                }
            };
            let _ = out_tx.send(HighlightResult {
                buffer_id: job.buffer_id,
                snapshot,
            });
        }
    }
}

fn highlight_job(
    buffers: &mut HashMap<usize, BufferState>,
    configs: &mut HashMap<LanguageId, HighlightConfiguration>,
    highlighter: &mut Highlighter,
    job: &HighlightRequest,
) -> anyhow::Result<HighlightSnapshot> {
    let language = job.language;
    let config = match configs.get(&language) {
        Some(cfg) => cfg,
        None => {
            warn!("no highlight config for {:?}", language);
            return Ok(HighlightSnapshot::plain(
                job.buffer_id,
                job.version,
                job.language,
                EngineKind::TreeSitter,
                SyntaxStatus::Error("no highlight config".into()),
                &job.text,
            ));
        }
    };

    let buffer_state = buffers.entry(job.buffer_id).or_insert_with(|| BufferState {
        language,
        parser: Parser::new(),
        tree: None,
    });

    if buffer_state.language != language {
        buffer_state.language = language;
        buffer_state.tree = None;
    }

    if let Some(lang) = LanguageRegistry::tree_sitter_language(language) {
        buffer_state.parser.set_language(lang)?;
    }

    let mut tree = if job.full_reparse || buffer_state.tree.is_none() {
        buffer_state.parser.parse(job.text.as_bytes(), None)
    } else {
        let mut existing = buffer_state.tree.take().unwrap();
        for edit in &job.edits {
            existing.edit(&to_input_edit(edit));
        }
        buffer_state
            .parser
            .parse(job.text.as_bytes(), Some(&existing))
            .or(Some(existing))
    };

    let Some(tree) = tree.take() else {
        return Ok(HighlightSnapshot::plain(
            job.buffer_id,
            job.version,
            job.language,
            EngineKind::TreeSitter,
            SyntaxStatus::Error("parse failed".into()),
            &job.text,
        ));
    };
    buffer_state.tree = Some(tree);

    let mut spans = Vec::new();
    let mut stack: Vec<HighlightGroup> = Vec::new();
    let mut iter = highlighter.highlight(
        config,
        job.text.as_bytes(),
        None,
        |name| {
            LanguageRegistry::from_injection_name(name)
                .and_then(|id| configs.get(&id))
        },
    )?;
    while let Some(event) = iter.next() {
        match event? {
            HighlightEvent::HighlightStart(s) => {
                let group = CAPTURE_GROUPS
                    .get(s.0)
                    .copied()
                    .unwrap_or(HighlightGroup::Normal);
                stack.push(group);
            }
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let group = stack.last().copied().unwrap_or(HighlightGroup::Normal);
                if group != HighlightGroup::Normal && end > start {
                    spans.push(HighlightSpan {
                        start_byte: start,
                        end_byte: end,
                        group,
                        priority: 0,
                        modifiers: 0,
                    });
                }
            }
        }
    }

    Ok(HighlightSnapshot::from_spans(
        job.buffer_id,
        job.version,
        job.language,
        EngineKind::TreeSitter,
        SyntaxStatus::Ok(EngineKind::TreeSitter),
        &job.text,
        spans,
        job.max_spans_per_line,
    ))
}

fn to_input_edit(edit: &BufferEdit) -> InputEdit {
    InputEdit {
        start_byte: edit.start_byte,
        old_end_byte: edit.old_end_byte,
        new_end_byte: edit.new_end_byte,
        start_position: to_point(edit.start_point),
        old_end_position: to_point(edit.old_end_point),
        new_end_position: to_point(edit.new_end_point),
    }
}

fn to_point(point: BufferPoint) -> Point {
    Point::new(point.row, point.column)
}

const CAPTURE_NAMES: &[&str] = &[
    "comment",
    "comment.documentation",
    "string",
    "string.special",
    "character",
    "number",
    "boolean",
    "keyword",
    "keyword.control",
    "keyword.operator",
    "type",
    "type.builtin",
    "function",
    "method",
    "macro",
    "attribute",
    "namespace",
    "variable",
    "parameter",
    "property",
    "constant",
    "operator",
    "punctuation",
    "tag",
    "heading",
    "emphasis",
    "link",
    "error",
    "warning",
    "constant.builtin",
    "function.macro",
    "function.method",
    "variable.parameter",
    "variable.builtin",
    "punctuation.bracket",
    "punctuation.delimiter",
    "constructor",
    "label",
    "escape",
];

const CAPTURE_GROUPS: &[HighlightGroup] = &[
    HighlightGroup::Comment,
    HighlightGroup::DocComment,
    HighlightGroup::String,
    HighlightGroup::String,
    HighlightGroup::Char,
    HighlightGroup::Number,
    HighlightGroup::Boolean,
    HighlightGroup::Keyword,
    HighlightGroup::KeywordControl,
    HighlightGroup::KeywordOperator,
    HighlightGroup::Type,
    HighlightGroup::TypeBuiltin,
    HighlightGroup::Function,
    HighlightGroup::Method,
    HighlightGroup::Macro,
    HighlightGroup::Attribute,
    HighlightGroup::Namespace,
    HighlightGroup::Variable,
    HighlightGroup::Parameter,
    HighlightGroup::Property,
    HighlightGroup::Constant,
    HighlightGroup::Operator,
    HighlightGroup::Punctuation,
    HighlightGroup::Tag,
    HighlightGroup::Heading,
    HighlightGroup::Emphasis,
    HighlightGroup::Link,
    HighlightGroup::Error,
    HighlightGroup::Warning,
    HighlightGroup::Number,
    HighlightGroup::Macro,
    HighlightGroup::Method,
    HighlightGroup::Parameter,
    HighlightGroup::Variable,
    HighlightGroup::Punctuation,
    HighlightGroup::Punctuation,
    HighlightGroup::Type,
    HighlightGroup::KeywordControl,
    HighlightGroup::String,
];

fn build_configs() -> HashMap<LanguageId, HighlightConfiguration> {
    let mut configs = HashMap::new();
    for lang in LanguageId::ALL {
        if let Some(ts_lang) = LanguageRegistry::tree_sitter_language(lang) {
            let highlights = LanguageRegistry::highlights_query(lang).unwrap_or("");
            let injections = LanguageRegistry::injections_query(lang);
            let mut config = match HighlightConfiguration::new(ts_lang, highlights, injections, "")
            {
                Ok(cfg) => cfg,
                Err(err) => {
                    warn!(
                        "failed to build highlight config for {:?} with injections: {err}",
                        lang
                    );
                    match HighlightConfiguration::new(ts_lang, highlights, "", "") {
                        Ok(cfg) => cfg,
                        Err(err) => {
                            warn!(
                                "failed to build highlight config for {:?}: {err}",
                                lang
                            );
                            continue;
                        }
                    }
                }
            };
            config.configure(CAPTURE_NAMES);
            configs.insert(lang, config);
        }
    }
    configs
}
