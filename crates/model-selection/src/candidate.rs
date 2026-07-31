//! Ranking of concrete model candidates (PRD §10).
//!
//! [`select_models`](crate::select_models) assigns *already qualified*
//! deployments to roles. This module answers the earlier question: given the raw
//! model names a provider enumerated, plus whatever metadata came with them,
//! which one should PurrCode code with?
//!
//! The ranking is deliberately built from evidence rather than from substring
//! guessing:
//!
//! * names are split into tokens (`qwen3-coder:30b` → `qwen3`, `coder`, `30b`)
//!   so `granite-embedding` is recognised as auxiliary while `granite-code` is
//!   recognised as a coder — a raw `contains("code")` cannot tell those apart;
//! * a model that cannot call tools is disqualifying, not merely unattractive,
//!   because the whole product is tool use;
//! * size is judged against the host's memory budget instead of "bigger is
//!   better", and the caller states whether it wants the largest model that
//!   fits or the smallest one that can still do the job;
//! * every comparison ends in a total order, so the same catalogue always picks
//!   the same model. Ranking by an unstable key is how a selector silently
//!   changes its mind between runs.

use serde::{Deserialize, Serialize};

/// What a model is built for, derived from its name tokens.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPurpose {
    /// Trained or tuned for code.
    Coding,
    /// A general instruction model; usable for code, not specialised.
    General,
    /// Embeddings, reranking, speech, vision, moderation. Never a coder.
    Auxiliary,
}

/// One model a provider offered, with whatever metadata the provider supplied.
/// Every metadata field is optional because providers disagree about what they
/// report; absent evidence must never be read as a negative signal.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModelCandidate {
    pub name: String,
    /// On-disk weight size in bytes, when the provider reports it (Ollama does).
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub context_tokens: Option<u32>,
    /// `Some(false)` only when tool calling was actually probed and failed.
    #[serde(default)]
    pub tool_calling: Option<bool>,
}

impl ModelCandidate {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    pub fn with_size(mut self, size_bytes: u64) -> Self {
        self.size_bytes = Some(size_bytes);
        self
    }

    pub fn purpose(&self) -> ModelPurpose {
        purpose_of(&self.name)
    }

    /// Parameter count in billions, parsed from a `30b` / `7B` / `3.8b` token.
    pub fn parameter_billions(&self) -> Option<f64> {
        tokens(&self.name).find_map(|token| {
            let digits = token.strip_suffix('b')?;
            let value: f64 = digits.parse().ok()?;
            (value > 0.0 && value < 10_000.0).then_some(value)
        })
    }

    /// Bytes of memory the weights need. Reported size wins; otherwise estimate
    /// from the parameter count at roughly 4-bit quantisation, which is what a
    /// local runtime serves by default.
    pub fn weight_footprint(&self) -> Option<u64> {
        self.size_bytes.or_else(|| {
            self.parameter_billions()
                .map(|billions| (billions * 600_000_000.0) as u64)
        })
    }

    /// How well this model suits the coding role, before size is considered.
    /// Auxiliary models are excluded by [`rank`] rather than scored.
    pub fn capability_score(&self) -> i64 {
        let mut score = match self.purpose() {
            ModelPurpose::Coding => 400,
            ModelPurpose::General => 150,
            ModelPurpose::Auxiliary => 0,
        };
        if tokens(&self.name).any(|token| matches!(token.as_str(), "instruct" | "chat" | "it")) {
            score += 40;
        }
        match self.tool_calling {
            // Proven tool calling outranks any name signal: a general model that
            // demonstrably calls tools beats a "coder" that demonstrably cannot.
            Some(true) => score += 200,
            Some(false) => score -= 600,
            None => {}
        }
        if let Some(context) = self.context_tokens {
            score += i64::from((context / 32_768).min(8)) * 15;
        }
        score
    }
}

/// Whether the caller wants the biggest capable model or the leanest one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizePreference {
    /// Capable host: take the largest model that fits the budget.
    LargestThatFits,
    /// Constrained host: take the smallest model that can still do the work.
    SmallestCapable,
}

/// The host constraint a candidate is ranked against.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SelectionBudget {
    /// Bytes available for model weights. `None` means unconstrained (a remote
    /// provider serves the weights, so the local host does not hold them).
    pub weight_bytes: Option<u64>,
    pub preference: SizePreference,
}

impl SelectionBudget {
    /// Budget for a host with `total_memory_bytes` of RAM. Weights get 70% of
    /// it; the rest is the working set of the editor, the daemon, and the build
    /// the agent is about to run. Below 16 GiB, prefer the leanest capable
    /// model so inference does not push the host into swap.
    pub fn for_local_host(total_memory_bytes: u64) -> Self {
        Self {
            weight_bytes: Some(total_memory_bytes / 10 * 7),
            preference: if total_memory_bytes <= 16 * 1024 * 1024 * 1024 {
                SizePreference::SmallestCapable
            } else {
                SizePreference::LargestThatFits
            },
        }
    }

    /// Budget for a provider that hosts the weights itself.
    pub fn remote() -> Self {
        Self {
            weight_bytes: None,
            preference: SizePreference::LargestThatFits,
        }
    }

    fn fits(&self, candidate: &ModelCandidate) -> bool {
        match (self.weight_bytes, candidate.weight_footprint()) {
            (Some(budget), Some(footprint)) => footprint <= budget,
            // An unknown size cannot be shown not to fit, and an unconstrained
            // budget fits everything.
            _ => true,
        }
    }
}

/// Rank candidates best-first for the coding role. Auxiliary models are removed
/// rather than ranked last: an embedding model is not a worse coder, it is not a
/// coder. Ties resolve through parameter count and then name, so the order is
/// total and stable across runs.
pub fn rank(candidates: &[ModelCandidate], budget: SelectionBudget) -> Vec<&ModelCandidate> {
    let mut ranked: Vec<&ModelCandidate> = candidates
        .iter()
        .filter(|candidate| candidate.purpose() != ModelPurpose::Auxiliary)
        .collect();
    ranked.sort_by(|left, right| {
        let key = |candidate: &ModelCandidate| {
            // Sorting ascending, so negate anything where "more is better".
            let footprint = candidate.weight_footprint();
            let size_key = match budget.preference {
                // Unknown sizes sort last in both directions: a model whose
                // footprint we can actually account for is the safer choice.
                SizePreference::LargestThatFits => u64::MAX - footprint.unwrap_or(0),
                SizePreference::SmallestCapable => footprint.unwrap_or(u64::MAX),
            };
            (
                -candidate.capability_score(),
                u8::from(!budget.fits(candidate)),
                size_key,
                candidate.name.clone(),
            )
        };
        key(left).cmp(&key(right))
    });
    ranked
}

/// The model PurrCode should code with, or `None` when the catalogue holds
/// nothing but auxiliary models.
pub fn select_coder(
    candidates: &[ModelCandidate],
    budget: SelectionBudget,
) -> Option<&ModelCandidate> {
    rank(candidates, budget).into_iter().next()
}

/// A second model for the judge role, distinct from `coder` when the catalogue
/// and the budget allow it. Independent judgment needs a different model; on a
/// constrained host, reusing one model is better than swapping two.
pub fn select_judge<'a>(
    candidates: &'a [ModelCandidate],
    budget: SelectionBudget,
    coder: &str,
) -> Option<&'a ModelCandidate> {
    if budget.preference == SizePreference::SmallestCapable {
        return None;
    }
    rank(candidates, budget)
        .into_iter()
        .find(|candidate| candidate.name != coder)
}

/// Lowercase alphanumeric tokens of a model name. Separators are anything that
/// is not a letter, a digit, or a decimal point, which splits registry paths,
/// tag suffixes, and vendor prefixes alike.
fn tokens(name: &str) -> impl Iterator<Item = String> + '_ {
    name.split(|character: char| !character.is_ascii_alphanumeric() && character != '.')
        .filter(|token| !token.is_empty())
        .map(|token| token.to_ascii_lowercase())
}

fn purpose_of(name: &str) -> ModelPurpose {
    const AUXILIARY: &[&str] = &[
        "embed",
        "embedding",
        "embeddings",
        "rerank",
        "reranker",
        "clip",
        "whisper",
        "tts",
        "stt",
        "guard",
        "guardrail",
        "moderation",
        "safety",
        "ocr",
        "diffusion",
        "sd",
        "flux",
    ];
    const CODING: &[&str] = &[
        "coder",
        "codestral",
        "codellama",
        "codegemma",
        "codegeex",
        "starcoder",
        "devstral",
        "sqlcoder",
        "deepcoder",
        "opencoder",
    ];

    let mut coding = false;
    for token in tokens(name) {
        if AUXILIARY.contains(&token.as_str()) {
            return ModelPurpose::Auxiliary;
        }
        // `code`, `code2`, `codegen`… but never `codec` style false friends,
        // which are caught by the auxiliary list above when they matter.
        if CODING.contains(&token.as_str()) || token.starts_with("code") {
            coding = true;
        }
    }
    if coding {
        ModelPurpose::Coding
    } else {
        ModelPurpose::General
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(ranked: &[&ModelCandidate]) -> Vec<String> {
        ranked.iter().map(|c| c.name.clone()).collect()
    }

    #[test]
    fn purpose_uses_tokens_not_substrings() {
        assert_eq!(
            purpose_of("granite-embedding:278m"),
            ModelPurpose::Auxiliary
        );
        assert_eq!(purpose_of("granite-code:8b"), ModelPurpose::Coding);
        assert_eq!(purpose_of("qwen3-coder:30b"), ModelPurpose::Coding);
        assert_eq!(purpose_of("nomic-embed-text"), ModelPurpose::Auxiliary);
        assert_eq!(purpose_of("llama3.3:70b"), ModelPurpose::General);
        // `deepseek` alone is a general family; only the coder variant codes.
        assert_eq!(purpose_of("deepseek-v3"), ModelPurpose::General);
        assert_eq!(purpose_of("deepseek-coder-v2"), ModelPurpose::Coding);
    }

    #[test]
    fn auxiliary_models_are_excluded_not_ranked_last() {
        let candidates = vec![
            ModelCandidate::new("nomic-embed-text"),
            ModelCandidate::new("qwen3-coder:30b"),
        ];
        let ranked = rank(&candidates, SelectionBudget::remote());
        assert_eq!(names(&ranked), vec!["qwen3-coder:30b"]);
    }

    #[test]
    fn only_auxiliary_models_selects_nothing() {
        let candidates = vec![ModelCandidate::new("bge-reranker")];
        assert!(select_coder(&candidates, SelectionBudget::remote()).is_none());
    }

    #[test]
    fn coding_models_outrank_general_models() {
        let candidates = vec![
            ModelCandidate::new("llama3.1:8b"),
            ModelCandidate::new("qwen2.5-coder:7b"),
        ];
        let chosen = select_coder(&candidates, SelectionBudget::remote()).unwrap();
        assert_eq!(chosen.name, "qwen2.5-coder:7b");
    }

    #[test]
    fn proven_tool_calling_outranks_a_coder_that_cannot_call_tools() {
        let candidates = vec![
            ModelCandidate {
                name: "some-coder:7b".into(),
                tool_calling: Some(false),
                ..Default::default()
            },
            ModelCandidate {
                name: "llama3.1:8b".into(),
                tool_calling: Some(true),
                ..Default::default()
            },
        ];
        let chosen = select_coder(&candidates, SelectionBudget::remote()).unwrap();
        assert_eq!(chosen.name, "llama3.1:8b");
    }

    #[test]
    fn unprobed_tool_calling_is_not_read_as_failure() {
        let probed = ModelCandidate {
            name: "a-coder".into(),
            tool_calling: Some(true),
            ..Default::default()
        };
        let unprobed = ModelCandidate::new("a-coder");
        assert!(probed.capability_score() > unprobed.capability_score());
        assert!(unprobed.capability_score() > 0);
    }

    #[test]
    fn capable_host_takes_the_largest_model_that_fits() {
        let candidates = vec![
            ModelCandidate::new("large").with_size(8_000),
            ModelCandidate::new("small").with_size(1_000),
            ModelCandidate::new("medium").with_size(4_000),
        ];
        let budget = SelectionBudget::for_local_host(64 * 1024 * 1024 * 1024);
        assert_eq!(
            select_coder(&candidates, budget).unwrap().name,
            "large",
            "equal capability must break toward the largest model that fits"
        );
    }

    #[test]
    fn constrained_host_takes_the_smallest_capable_model() {
        let candidates = vec![
            ModelCandidate::new("large").with_size(8_000),
            ModelCandidate::new("small").with_size(1_000),
            ModelCandidate::new("medium").with_size(4_000),
        ];
        let budget = SelectionBudget::for_local_host(8 * 1024 * 1024 * 1024);
        assert_eq!(select_coder(&candidates, budget).unwrap().name, "small");
    }

    #[test]
    fn a_model_over_budget_loses_to_one_that_fits() {
        let candidates = vec![
            ModelCandidate::new("huge-coder:70b").with_size(40 * 1024 * 1024 * 1024),
            ModelCandidate::new("small-coder:7b").with_size(4 * 1024 * 1024 * 1024),
        ];
        let budget = SelectionBudget {
            weight_bytes: Some(8 * 1024 * 1024 * 1024),
            preference: SizePreference::LargestThatFits,
        };
        assert_eq!(
            select_coder(&candidates, budget).unwrap().name,
            "small-coder:7b"
        );
    }

    #[test]
    fn over_budget_models_still_rank_when_nothing_fits() {
        let candidates = vec![ModelCandidate::new("huge-coder:70b").with_size(40_000_000_000)];
        let budget = SelectionBudget {
            weight_bytes: Some(1_000),
            preference: SizePreference::LargestThatFits,
        };
        assert_eq!(
            select_coder(&candidates, budget).unwrap().name,
            "huge-coder:70b",
            "a model that cannot fit is still better than refusing to start"
        );
    }

    #[test]
    fn unknown_sizes_fall_back_to_a_stable_name_order() {
        let candidates = vec![ModelCandidate::new("first"), ModelCandidate::new("second")];
        for preference in [
            SizePreference::LargestThatFits,
            SizePreference::SmallestCapable,
        ] {
            let budget = SelectionBudget {
                weight_bytes: None,
                preference,
            };
            assert_eq!(select_coder(&candidates, budget).unwrap().name, "first");
        }
    }

    #[test]
    fn a_known_size_is_preferred_over_an_unknown_one_at_equal_capability() {
        let candidates = vec![
            ModelCandidate::new("aaa-unknown"),
            ModelCandidate::new("zzz-known").with_size(4_000_000_000),
        ];
        for preference in [
            SizePreference::LargestThatFits,
            SizePreference::SmallestCapable,
        ] {
            let budget = SelectionBudget {
                weight_bytes: Some(64_000_000_000),
                preference,
            };
            assert_eq!(select_coder(&candidates, budget).unwrap().name, "zzz-known");
        }
    }

    #[test]
    fn ranking_is_total_and_independent_of_input_order() {
        let build = || {
            vec![
                ModelCandidate::new("qwen3-coder:30b").with_size(18_000_000_000),
                ModelCandidate::new("llama3.1:8b").with_size(4_700_000_000),
                ModelCandidate::new("qwen2.5-coder:7b").with_size(4_400_000_000),
                ModelCandidate::new("nomic-embed-text").with_size(270_000_000),
            ]
        };
        let budget = SelectionBudget::for_local_host(64 * 1024 * 1024 * 1024);
        let forward = build();
        let mut reversed = build();
        reversed.reverse();
        assert_eq!(
            names(&rank(&forward, budget)),
            names(&rank(&reversed, budget))
        );
        assert_eq!(
            names(&rank(&forward, budget)),
            vec!["qwen3-coder:30b", "qwen2.5-coder:7b", "llama3.1:8b"]
        );
    }

    #[test]
    fn parameter_count_is_parsed_and_estimates_a_footprint() {
        assert_eq!(
            ModelCandidate::new("qwen3-coder:30b").parameter_billions(),
            Some(30.0)
        );
        assert_eq!(
            ModelCandidate::new("llama3.2:3.8B").parameter_billions(),
            Some(3.8)
        );
        assert_eq!(ModelCandidate::new("gpt-5").parameter_billions(), None);
        // 30B at ~4-bit is ~18 GB of weights.
        let footprint = ModelCandidate::new("qwen3-coder:30b")
            .weight_footprint()
            .unwrap();
        assert_eq!(footprint, 18_000_000_000);
    }

    #[test]
    fn reported_size_wins_over_the_estimate() {
        let candidate = ModelCandidate::new("qwen3-coder:30b").with_size(19_500_000_000);
        assert_eq!(candidate.weight_footprint(), Some(19_500_000_000));
    }

    #[test]
    fn context_window_breaks_ties_between_equal_families() {
        let short = ModelCandidate {
            name: "model-a".into(),
            context_tokens: Some(8_192),
            ..Default::default()
        };
        let long = ModelCandidate {
            name: "model-b".into(),
            context_tokens: Some(262_144),
            ..Default::default()
        };
        assert!(long.capability_score() > short.capability_score());
    }

    #[test]
    fn judge_is_distinct_on_a_capable_host_and_shared_when_constrained() {
        let candidates = vec![
            ModelCandidate::new("qwen3-coder:30b").with_size(18_000_000_000),
            ModelCandidate::new("llama3.1:8b").with_size(4_700_000_000),
        ];
        let capable = SelectionBudget::for_local_host(64 * 1024 * 1024 * 1024);
        let coder = select_coder(&candidates, capable).unwrap();
        assert_eq!(coder.name, "qwen3-coder:30b");
        assert_eq!(
            select_judge(&candidates, capable, &coder.name)
                .unwrap()
                .name,
            "llama3.1:8b"
        );
        let constrained = SelectionBudget::for_local_host(8 * 1024 * 1024 * 1024);
        assert!(select_judge(&candidates, constrained, "anything").is_none());
    }
}
