//! Compiling a prompt from typed sections, with the cost of each recorded.
//!
//! A prompt was built by concatenating strings at the call site: the system
//! prompt, a per-model suffix, a session ledger, a block of repository
//! excerpts and the task, glued together and handed over. Two things followed
//! from that shape.
//!
//! The repository excerpts and the task shared one user message, so nothing
//! downstream could tell them apart -- not compaction, which had to keep the
//! whole thing or lose the goal with it, and not a reader asking what a turn
//! actually cost. And the budget was a fraction: retrieval got a share of the
//! context and nobody knew what any section spent, so a prompt that was too
//! large could only be made smaller by guessing which part to cut.
//!
//! Sections are typed here, each carries its own estimated cost and hash, and
//! what was dropped or truncated to make the prompt fit is recorded rather
//! than inferred.

use poorai_domain::{ChatMessage, hash_bytes};
use serde::{Deserialize, Serialize};

/// What a section is, which is also what may be done to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    /// The shared agent instructions. Required.
    System,
    /// A per-deployment addition to them. Required with the system prompt,
    /// because a deployment told to behave one way and then not told is being
    /// given two different agents on two turns.
    ModelSuffix,
    /// The task. Required, and the last thing that would ever be dropped: a
    /// run without its goal is not a cheaper run, it is a different one.
    Task,
    /// What earlier runs of this session established.
    SessionLedger,
    /// Passages ranked against the task.
    RepositoryExcerpts,
}

impl SectionKind {
    /// Whether the prompt is still the prompt without it.
    pub fn required(self) -> bool {
        matches!(self, Self::System | Self::ModelSuffix | Self::Task)
    }

    /// What gets cut first. Higher drops earlier.
    ///
    /// Excerpts before the ledger: excerpts are a starting point the agent can
    /// rebuild with `search` and `read_file`, while the ledger is the only
    /// account of what earlier runs did and cannot be recovered from the
    /// workspace.
    fn eviction_order(self) -> u8 {
        match self {
            Self::RepositoryExcerpts => 0,
            Self::SessionLedger => 1,
            _ => u8::MAX,
        }
    }

    fn role(self) -> &'static str {
        match self {
            Self::System | Self::ModelSuffix => "system",
            _ => "user",
        }
    }
}

/// A section offered to the compiler.
#[derive(Debug, Clone)]
pub struct Section {
    pub kind: SectionKind,
    pub content: String,
}

impl Section {
    pub fn new(kind: SectionKind, content: impl Into<String>) -> Self {
        Self {
            kind,
            content: content.into(),
        }
    }
}

/// What happened to a section, and what it cost.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompiledSection {
    pub kind: SectionKind,
    /// Estimated at four characters per token, and labelled as an estimate
    /// wherever it is read. The backend's reported count is compared against
    /// the total after the turn.
    pub estimated_tokens: usize,
    pub bytes: usize,
    pub content_hash: String,
    /// Cut to fit, with the number of characters that survived.
    pub truncated_to: Option<usize>,
    /// Left out entirely.
    pub dropped: bool,
}

/// A prompt, with the accounting that produced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledPrompt {
    pub sections: Vec<CompiledSection>,
    pub estimated_tokens: usize,
    /// Held back for the reply. A prompt that fills the context leaves the
    /// deployment nowhere to answer, which is a failure that looks like a
    /// refusal.
    pub reserve_tokens: usize,
    pub context_tokens: u32,
    /// True when something had to be cut. Recorded so a run whose retrieval
    /// was thrown away does not look like one that was never offered any.
    pub reduced: bool,
}

/// Characters per token. An estimate, and never reported as a count.
const CHARS_PER_TOKEN: usize = 4;

/// The share of the context held back for the reply.
///
/// A quota, and the honest note is that it is not yet a measured one: the
/// audit asked for measured quotas and this is a starting value, replaced when
/// a campaign has enough reply lengths to derive one. It is here rather than
/// scattered because a number in one place can be measured; five numbers at
/// five call sites cannot.
const OUTPUT_RESERVE_SHARE: f64 = 0.25;

/// A truncated section keeps at least this much, or is dropped instead.
///
/// A section cut to a few hundred characters is not a smaller section, it is a
/// misleading one -- half an excerpt reads like a whole file.
const MIN_USEFUL_CHARS: usize = 2_000;

pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(CHARS_PER_TOKEN)
}

/// Fits the sections into the context and says what that cost.
///
/// Required sections are never cut. Optional ones are dropped in eviction
/// order until the rest fits, and the last one considered is truncated rather
/// than dropped if what survives is still worth reading.
pub fn compile(sections: Vec<Section>, context_tokens: u32) -> (Vec<ChatMessage>, CompiledPrompt) {
    let reserve_tokens = (f64::from(context_tokens) * OUTPUT_RESERVE_SHARE) as usize;
    let budget = (context_tokens as usize).saturating_sub(reserve_tokens);

    let mut sections = sections;
    sections.retain(|section| !section.content.is_empty());

    let required: usize = sections
        .iter()
        .filter(|section| section.kind.required())
        .map(|section| estimate_tokens(&section.content))
        .sum();

    // Optional sections, worst first, so the one evicted is the one whose
    // absence costs least.
    let mut order: Vec<usize> = (0..sections.len())
        .filter(|index| !sections[*index].kind.required())
        .collect();
    order.sort_by_key(|index| sections[*index].kind.eviction_order());

    let mut kept: std::collections::BTreeMap<usize, Option<usize>> =
        order.iter().map(|index| (*index, None::<usize>)).collect();
    let spend = |kept: &std::collections::BTreeMap<usize, Option<usize>>| -> usize {
        required
            + kept
                .iter()
                .map(|(index, limit)| match limit {
                    Some(chars) => estimate_tokens(&sections[*index].content[..*chars]),
                    None => estimate_tokens(&sections[*index].content),
                })
                .sum::<usize>()
    };

    for index in &order {
        if spend(&kept) <= budget {
            break;
        }
        // Try to keep some of it before giving it up entirely.
        let over = spend(&kept).saturating_sub(budget);
        let content = &sections[*index].content;
        let keep_chars = content.len().saturating_sub(over * CHARS_PER_TOKEN);
        let keep_chars = floor_char_boundary(content, keep_chars);
        if keep_chars >= MIN_USEFUL_CHARS {
            kept.insert(*index, Some(keep_chars));
        } else {
            kept.remove(index);
        }
    }

    let mut messages: Vec<ChatMessage> = Vec::new();
    let mut compiled: Vec<CompiledSection> = Vec::new();
    let mut reduced = false;
    for (index, section) in sections.iter().enumerate() {
        let limit = if section.kind.required() {
            Some(None)
        } else {
            kept.get(&index).copied()
        };
        let Some(limit) = limit else {
            reduced = true;
            compiled.push(CompiledSection {
                kind: section.kind,
                estimated_tokens: 0,
                bytes: 0,
                content_hash: hash_bytes(section.content.as_bytes()),
                truncated_to: None,
                dropped: true,
            });
            continue;
        };
        let content = match limit {
            Some(chars) => {
                reduced = true;
                &section.content[..chars]
            }
            None => section.content.as_str(),
        };
        compiled.push(CompiledSection {
            kind: section.kind,
            estimated_tokens: estimate_tokens(content),
            bytes: content.len(),
            // The hash of what was sent, not of what was offered, so a
            // truncated section is not mistaken for the whole one.
            content_hash: hash_bytes(content.as_bytes()),
            truncated_to: limit,
            dropped: false,
        });
        messages.push(ChatMessage {
            role: section.kind.role().into(),
            content: content.to_string(),
        });
    }

    // The system prompt and its per-deployment suffix become one message,
    // because they are one instruction and a backend that expects a single
    // system message should get one.
    //
    // User sections are never merged. Gluing the repository excerpts to the
    // task is exactly the shape this file exists to undo: nothing downstream
    // could tell them apart, so compaction had to keep the whole thing or lose
    // the goal with it.
    let messages = merge_system(messages);
    let estimated_tokens = compiled
        .iter()
        .map(|section| section.estimated_tokens)
        .sum();
    (
        messages,
        CompiledPrompt {
            sections: compiled,
            estimated_tokens,
            reserve_tokens,
            context_tokens,
            reduced,
        },
    )
}

fn merge_system(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut merged: Vec<ChatMessage> = Vec::new();
    for message in messages {
        match merged.last_mut() {
            Some(last) if last.role == "system" && message.role == "system" => {
                last.content.push_str(&message.content);
            }
            _ => merged.push(message),
        }
    }
    merged
}

/// The largest index at or below `at` that is a character boundary.
///
/// Cutting a string mid-code-point panics, and a prompt that panics on a
/// non-ASCII repository is worse than one that keeps four bytes fewer.
fn floor_char_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}
