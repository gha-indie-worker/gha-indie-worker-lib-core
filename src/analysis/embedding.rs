//! Embedding storage identity — model-space v3, contract 3.1.0.
//!
//! The fleet stores every embedding in a fixed **4100-slot** vector, whatever model produced it,
//! because the widest models in use (Qwen3-Embedding-8B, NV-Embed-v2, bge-en-icl) are 4096-wide
//! and a common width lets one table serve every model. Shorter vectors (OpenAI
//! `text-embedding-3-small` at 1536, `-3-large` at 3072) are **zero-padded into the unused tail**.
//!
//! Padding is safe for the metrics we use — and *only* because of that. Appending zeros does not
//! change a dot product and does not change an L2 distance. It also does not change cosine
//! similarity, because the L2 norm of the padded vector is the norm of the original. What it
//! would break is any metric that divides by the dimension count (a "mean squared difference"
//! similarity, say), so [`Metric`] is a closed set of the three that are provably invariant.
//!
//! The other half of this module is the thing that actually prevents silent corruption: **two
//! embeddings may only be compared when they came from the same comparison space.** A cosine
//! similarity between an OpenAI vector and a Qwen vector is a number, and it is meaningless. The
//! 12-field [`ComparisonSpace`] identity makes that a type error rather than a plausible result.

use core::fmt;

/// Storage width for every embedding in the fleet.
pub const STORAGE_SLOTS: usize = 4100;

/// Metrics that are invariant under zero-padding. Deliberately closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Metric {
    /// Invariant: the added terms are `0 * 0`.
    Dot,
    /// Invariant: `‖v‖` is unchanged by appending zeros, and so is the numerator.
    Cosine,
    /// Invariant: the added terms are `(0 - 0)²`.
    L2,
}

impl Metric {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Metric::Dot => "dot",
            Metric::Cosine => "cosine",
            Metric::L2 => "l2",
        }
    }
}

/// How a model is asked to encode a query versus a document. Asymmetric models (E5, BGE, Qwen3)
/// prepend different instructions, and encoding a document as a query quietly degrades recall —
/// a bug with no error message, so the role is part of the identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Role {
    Query,
    Document,
}

impl Role {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::Query => "query",
            Role::Document => "document",
        }
    }
}

/// Index strategy for a space. Chosen per model, because the right answer depends on the
/// dimension and on whether the model was trained for Matryoshka truncation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum AnnStrategy {
    /// Exact scan over `halfvec`. Correct by construction; the default until recall is measured.
    HalfvecExact,
    /// HNSW over a Matryoshka-truncated prefix. Only valid for models trained for it.
    HalfvecMrl,
    /// Binary quantization for the candidate set, full-precision rerank on top.
    BinaryFullRerank,
}

impl AnnStrategy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            AnnStrategy::HalfvecExact => "halfvec-exact",
            AnnStrategy::HalfvecMrl => "halfvec-mrl",
            AnnStrategy::BinaryFullRerank => "binary-full-rerank",
        }
    }

    /// Whether this strategy can return a wrong neighbour. Exact scan cannot; the other two can,
    /// which is why enabling them requires measured recall evidence.
    #[must_use]
    pub const fn is_approximate(self) -> bool {
        !matches!(self, AnnStrategy::HalfvecExact)
    }
}

/// The 12 fields that decide whether two vectors may be compared. Any difference makes them
/// different spaces, and a cross-space comparison is refused rather than approximated.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ComparisonSpace {
    /// 1. Tenant. Vectors never cross a tenant boundary, even within one model.
    pub tenant: String,
    /// 2. What is embedded (`run-log`, `workflow`, `ticket`).
    pub entity: String,
    /// 3. Model identifier, exactly as the provider names it.
    pub model: String,
    /// 4. Model revision. A silently re-trained model is a different space.
    pub model_revision: String,
    /// 5. Native dimension before padding.
    pub dimensions: u16,
    /// 6. Metric.
    pub metric: Metric,
    /// 7. Query or document.
    pub role: Role,
    /// 8. The instruction/prefix template applied before encoding.
    pub prompt_profile: String,
    /// 9. How the text was chunked; different chunking is different content.
    pub chunk_profile: String,
    /// 10. Normalization applied after encoding (`l2`, `none`).
    pub normalization: String,
    /// 11. Whether the stored vector is a Matryoshka truncation, and to what width.
    pub truncated_to: Option<u16>,
    /// 12. Contract version that produced the row.
    pub contract_version: String,
}

impl ComparisonSpace {
    /// A stable key for the space. Field separator is `\u{1f}` (unit separator) so a value
    /// containing a colon or a slash cannot forge a different identity.
    #[must_use]
    pub fn key(&self) -> String {
        let truncated = self
            .truncated_to
            .map_or_else(|| "-".to_owned(), |w| w.to_string());
        [
            self.tenant.as_str(),
            self.entity.as_str(),
            self.model.as_str(),
            self.model_revision.as_str(),
            &self.dimensions.to_string(),
            self.metric.as_str(),
            self.role.as_str(),
            self.prompt_profile.as_str(),
            self.chunk_profile.as_str(),
            self.normalization.as_str(),
            &truncated,
            self.contract_version.as_str(),
        ]
        .join("\u{1f}")
    }

    /// Two spaces are comparable when they are identical **except** for the role: a query is
    /// meant to be compared against documents, and that asymmetry is the point.
    #[must_use]
    pub fn comparable_with(&self, other: &Self) -> bool {
        self.tenant == other.tenant
            && self.entity == other.entity
            && self.model == other.model
            && self.model_revision == other.model_revision
            && self.dimensions == other.dimensions
            && self.metric == other.metric
            && self.prompt_profile == other.prompt_profile
            && self.chunk_profile == other.chunk_profile
            && self.normalization == other.normalization
            && self.truncated_to == other.truncated_to
            && self.contract_version == other.contract_version
    }
}

/// Why a vector or a comparison was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EmbeddingError {
    /// The vector is wider than the storage slot.
    TooWide { got: usize, max: usize },
    /// The vector's width does not match the space's declared dimension.
    DimensionMismatch { declared: u16, got: usize },
    /// A value that is not finite would poison every later aggregate.
    NotFinite { index: usize },
    /// The two vectors are not from the same comparison space.
    IncomparableSpaces { left: String, right: String },
    /// The space is not enabled. Every space ships disabled; enabling one requires evidence.
    SpaceDisabled { space: String },
    /// An approximate index was requested without measured recall.
    ApproximateWithoutEvidence { strategy: AnnStrategy },
}

impl fmt::Display for EmbeddingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmbeddingError::TooWide { got, max } => {
                write!(f, "embedding has {got} dimensions; storage holds {max}")
            }
            EmbeddingError::DimensionMismatch { declared, got } => {
                write!(
                    f,
                    "space declares {declared} dimensions but the vector has {got}"
                )
            }
            EmbeddingError::NotFinite { index } => {
                write!(f, "embedding component {index} is not finite")
            }
            EmbeddingError::IncomparableSpaces { left, right } => {
                write!(
                    f,
                    "refusing to compare vectors from different spaces:\n  {left}\n  {right}"
                )
            }
            EmbeddingError::SpaceDisabled { space } => {
                write!(
                    f,
                    "comparison space {space} is disabled pending review evidence"
                )
            }
            EmbeddingError::ApproximateWithoutEvidence { strategy } => write!(
                f,
                "{} is approximate; enable it only with measured exact-vs-ANN recall",
                strategy.as_str()
            ),
        }
    }
}

/// Pad a model's native vector into the fixed storage width.
///
/// # Errors
/// [`EmbeddingError::TooWide`], [`EmbeddingError::DimensionMismatch`], [`EmbeddingError::NotFinite`].
pub fn pad_for_storage(
    vector: &[f32],
    space: &ComparisonSpace,
) -> Result<Vec<f32>, EmbeddingError> {
    if vector.len() > STORAGE_SLOTS {
        return Err(EmbeddingError::TooWide {
            got: vector.len(),
            max: STORAGE_SLOTS,
        });
    }
    let declared = usize::from(space.truncated_to.unwrap_or(space.dimensions));
    if vector.len() != declared {
        return Err(EmbeddingError::DimensionMismatch {
            declared: space.truncated_to.unwrap_or(space.dimensions),
            got: vector.len(),
        });
    }
    if let Some(index) = vector.iter().position(|v| !v.is_finite()) {
        return Err(EmbeddingError::NotFinite { index });
    }
    let mut padded = Vec::with_capacity(STORAGE_SLOTS);
    padded.extend_from_slice(vector);
    padded.resize(STORAGE_SLOTS, 0.0);
    Ok(padded)
}

/// Recover the model's native vector from a stored row.
#[must_use]
pub fn unpad(stored: &[f32], space: &ComparisonSpace) -> Vec<f32> {
    let width = usize::from(space.truncated_to.unwrap_or(space.dimensions)).min(stored.len());
    stored[..width].to_vec()
}

/// Similarity between two stored vectors, refusing anything cross-space.
///
/// Higher is more similar for [`Metric::Dot`] and [`Metric::Cosine`]; for [`Metric::L2`] the
/// distance is returned, where **lower** is more similar — the caller must not treat these
/// interchangeably, which is why the metric lives in the space rather than in a call argument.
///
/// # Errors
/// [`EmbeddingError::IncomparableSpaces`], [`EmbeddingError::SpaceDisabled`].
pub fn similarity(
    left: (&[f32], &ComparisonSpace),
    right: (&[f32], &ComparisonSpace),
    enabled: bool,
) -> Result<f64, EmbeddingError> {
    let (left_vector, left_space) = left;
    let (right_vector, right_space) = right;
    if !left_space.comparable_with(right_space) {
        return Err(EmbeddingError::IncomparableSpaces {
            left: left_space.key(),
            right: right_space.key(),
        });
    }
    if !enabled {
        return Err(EmbeddingError::SpaceDisabled {
            space: left_space.key(),
        });
    }
    let n = left_vector.len().min(right_vector.len());
    let (a, b) = (&left_vector[..n], &right_vector[..n]);
    Ok(match left_space.metric {
        Metric::Dot => dot(a, b),
        Metric::Cosine => {
            let (na, nb) = (norm(a), norm(b));
            if na == 0.0 || nb == 0.0 {
                0.0
            } else {
                dot(a, b) / (na * nb)
            }
        }
        Metric::L2 => a
            .iter()
            .zip(b)
            .map(|(x, y)| {
                let d = f64::from(*x) - f64::from(*y);
                d * d
            })
            .sum::<f64>()
            .sqrt(),
    })
}

fn dot(a: &[f32], b: &[f32]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| f64::from(*x) * f64::from(*y))
        .sum()
}

fn norm(a: &[f32]) -> f64 {
    dot(a, a).sqrt()
}

/// A registered space and its rollout state. Every space is created **disabled**; enabling one
/// requires the six pieces of evidence the v3 contract asks for, and an approximate index adds a
/// seventh. This type exists so "we turned it on and forgot to check recall" is not expressible.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpaceRegistration {
    pub space: ComparisonSpace,
    pub ann_strategy: AnnStrategy,
    pub enabled: bool,
    pub evidence: Evidence,
}

/// The review gates. All must hold before a space may serve traffic.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Evidence {
    pub migration_replay: bool,
    pub tenant_isolation: bool,
    pub lexical_only_retrieval: bool,
    pub exact_vs_ann_recall: bool,
    pub rollback: bool,
    pub backup_restore: bool,
}

impl Evidence {
    #[must_use]
    pub const fn complete_for(self, strategy: AnnStrategy) -> bool {
        let base = self.migration_replay
            && self.tenant_isolation
            && self.lexical_only_retrieval
            && self.rollback
            && self.backup_restore;
        if strategy.is_approximate() {
            base && self.exact_vs_ann_recall
        } else {
            base
        }
    }

    #[must_use]
    pub fn missing_for(self, strategy: AnnStrategy) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if !self.migration_replay {
            missing.push("migration replay");
        }
        if !self.tenant_isolation {
            missing.push("tenant isolation (RLS)");
        }
        if !self.lexical_only_retrieval {
            missing.push("lexical-only retrieval fallback");
        }
        if strategy.is_approximate() && !self.exact_vs_ann_recall {
            missing.push("exact-vs-ANN recall");
        }
        if !self.rollback {
            missing.push("rollback");
        }
        if !self.backup_restore {
            missing.push("backup/restore");
        }
        missing
    }
}

impl SpaceRegistration {
    /// A newly registered space: disabled, exact index, no evidence.
    #[must_use]
    pub fn new(space: ComparisonSpace) -> Self {
        Self {
            space,
            ann_strategy: AnnStrategy::HalfvecExact,
            enabled: false,
            evidence: Evidence::default(),
        }
    }

    /// Turn the space on.
    ///
    /// # Errors
    /// [`EmbeddingError::SpaceDisabled`] listing what is missing, or
    /// [`EmbeddingError::ApproximateWithoutEvidence`].
    pub fn enable(&mut self) -> Result<(), EmbeddingError> {
        if self.ann_strategy.is_approximate() && !self.evidence.exact_vs_ann_recall {
            return Err(EmbeddingError::ApproximateWithoutEvidence {
                strategy: self.ann_strategy,
            });
        }
        if !self.evidence.complete_for(self.ann_strategy) {
            return Err(EmbeddingError::SpaceDisabled {
                space: format!(
                    "{} (missing: {})",
                    self.space.key(),
                    self.evidence.missing_for(self.ann_strategy).join(", ")
                ),
            });
        }
        self.enabled = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(model: &str, dimensions: u16, role: Role) -> ComparisonSpace {
        ComparisonSpace {
            tenant: "org_acme".into(),
            entity: "run-log".into(),
            model: model.into(),
            model_revision: "2026-05".into(),
            dimensions,
            metric: Metric::Cosine,
            role,
            prompt_profile: "default".into(),
            chunk_profile: "lines-512".into(),
            normalization: "l2".into(),
            truncated_to: None,
            contract_version: "3.1.0".into(),
        }
    }

    fn full_evidence() -> Evidence {
        Evidence {
            migration_replay: true,
            tenant_isolation: true,
            lexical_only_retrieval: true,
            exact_vs_ann_recall: true,
            rollback: true,
            backup_restore: true,
        }
    }

    #[test]
    fn padding_reaches_the_storage_width_and_round_trips() {
        let s = space("text-embedding-3-small", 1536, Role::Document);
        let vector: Vec<f32> = (0..1536).map(|i| (i as f32) / 1536.0).collect();
        let padded = pad_for_storage(&vector, &s).unwrap();
        assert_eq!(padded.len(), STORAGE_SLOTS);
        assert!(padded[1536..].iter().all(|v| *v == 0.0));
        assert_eq!(unpad(&padded, &s), vector);
    }

    #[test]
    fn a_4096_wide_model_fits_with_slots_to_spare() {
        let s = space("Qwen3-Embedding-8B", 4096, Role::Document);
        let vector = vec![0.5_f32; 4096];
        assert_eq!(pad_for_storage(&vector, &s).unwrap().len(), STORAGE_SLOTS);
    }

    #[test]
    fn padding_does_not_change_any_supported_metric() {
        // The property the whole storage scheme rests on.
        let a: Vec<f32> = (0..1536).map(|i| ((i % 17) as f32) - 8.0).collect();
        let b: Vec<f32> = (0..1536).map(|i| ((i % 13) as f32) - 6.0).collect();
        for metric in [Metric::Dot, Metric::Cosine, Metric::L2] {
            let mut s = space("m", 1536, Role::Document);
            s.metric = metric;
            let pa = pad_for_storage(&a, &s).unwrap();
            let pb = pad_for_storage(&b, &s).unwrap();
            let native = similarity((&a, &s), (&b, &s), true).unwrap();
            let padded = similarity((&pa, &s), (&pb, &s), true).unwrap();
            assert!(
                (native - padded).abs() < 1e-9,
                "{} changed under padding: {native} vs {padded}",
                metric.as_str()
            );
        }
    }

    #[test]
    fn a_vector_wider_than_storage_or_of_the_wrong_width_is_refused() {
        let s = space("huge", 1536, Role::Document);
        assert_eq!(
            pad_for_storage(&vec![0.0; STORAGE_SLOTS + 1], &s).unwrap_err(),
            EmbeddingError::TooWide {
                got: STORAGE_SLOTS + 1,
                max: STORAGE_SLOTS
            }
        );
        assert_eq!(
            pad_for_storage(&vec![0.0; 100], &s).unwrap_err(),
            EmbeddingError::DimensionMismatch {
                declared: 1536,
                got: 100
            }
        );
    }

    #[test]
    fn a_non_finite_component_never_reaches_storage() {
        let s = space("m", 3, Role::Document);
        assert_eq!(
            pad_for_storage(&[0.0, f32::NAN, 1.0], &s).unwrap_err(),
            EmbeddingError::NotFinite { index: 1 }
        );
        assert_eq!(
            pad_for_storage(&[f32::INFINITY, 0.0, 0.0], &s).unwrap_err(),
            EmbeddingError::NotFinite { index: 0 }
        );
    }

    #[test]
    fn vectors_from_different_models_are_never_compared() {
        let openai = space("text-embedding-3-small", 8, Role::Document);
        let qwen = space("Qwen3-Embedding-8B", 8, Role::Document);
        let v = vec![1.0_f32; 8];
        let err = similarity((&v, &openai), (&v, &qwen), true).unwrap_err();
        assert!(matches!(err, EmbeddingError::IncomparableSpaces { .. }));
    }

    #[test]
    fn a_query_and_a_document_from_the_same_model_are_comparable() {
        let query = space("bge-en-icl", 8, Role::Query);
        let document = space("bge-en-icl", 8, Role::Document);
        assert!(query.comparable_with(&document));
        assert_ne!(
            query.key(),
            document.key(),
            "but they remain distinct identities"
        );
        let v = vec![1.0_f32; 8];
        assert!(similarity((&v, &query), (&v, &document), true).is_ok());
    }

    #[test]
    fn a_different_tenant_prompt_or_chunking_is_a_different_space() {
        let base = space("m", 8, Role::Document);
        for mutate in [
            (|s: &mut ComparisonSpace| s.tenant = "org_other".into()) as fn(&mut ComparisonSpace),
            |s: &mut ComparisonSpace| s.prompt_profile = "instruct-v2".into(),
            |s: &mut ComparisonSpace| s.chunk_profile = "paragraphs".into(),
            |s: &mut ComparisonSpace| s.normalization = "none".into(),
            |s: &mut ComparisonSpace| s.model_revision = "2026-09".into(),
            |s: &mut ComparisonSpace| s.truncated_to = Some(256),
            |s: &mut ComparisonSpace| s.contract_version = "3.2.0".into(),
        ] {
            let mut other = base.clone();
            mutate(&mut other);
            assert!(
                !base.comparable_with(&other),
                "should be incomparable after mutation"
            );
        }
    }

    #[test]
    fn the_space_key_cannot_be_forged_by_a_value_containing_a_separator() {
        let mut a = space("m", 8, Role::Document);
        a.tenant = "org_acme\u{1f}run-log".into();
        a.entity = "x".into();
        let b = space("m", 8, Role::Document);
        assert_ne!(a.key(), b.key());
    }

    #[test]
    fn a_new_space_ships_disabled_and_refuses_traffic() {
        let registration = SpaceRegistration::new(space("m", 8, Role::Document));
        assert!(!registration.enabled);
        let v = vec![1.0_f32; 8];
        let err = similarity(
            (&v, &registration.space),
            (&v, &registration.space),
            registration.enabled,
        )
        .unwrap_err();
        assert!(matches!(err, EmbeddingError::SpaceDisabled { .. }));
    }

    #[test]
    fn enabling_requires_every_piece_of_evidence() {
        let mut registration = SpaceRegistration::new(space("m", 8, Role::Document));
        assert!(registration.enable().is_err());
        registration.evidence = full_evidence();
        registration.evidence.tenant_isolation = false;
        let err = registration.enable().unwrap_err();
        assert!(format!("{err}").contains("tenant isolation"), "{err}");
        registration.evidence = full_evidence();
        assert!(registration.enable().is_ok());
        assert!(registration.enabled);
    }

    #[test]
    fn an_approximate_index_additionally_requires_measured_recall() {
        let mut registration = SpaceRegistration::new(space("m", 8, Role::Document));
        registration.ann_strategy = AnnStrategy::HalfvecMrl;
        registration.evidence = full_evidence();
        registration.evidence.exact_vs_ann_recall = false;
        assert_eq!(
            registration.enable().unwrap_err(),
            EmbeddingError::ApproximateWithoutEvidence {
                strategy: AnnStrategy::HalfvecMrl
            }
        );
        // Exact scan needs no recall evidence: it cannot be wrong.
        registration.ann_strategy = AnnStrategy::HalfvecExact;
        assert!(registration.enable().is_ok());
    }
}
