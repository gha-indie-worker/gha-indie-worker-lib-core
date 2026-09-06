//! Correlation, regression and change-point detection — the "why did this get slower / start
//! failing" engine.
//!
//! Three questions this answers, all of which come up daily on a CI/CD platform:
//!
//! 1. **Correlation** — of the fifty things we record about a run (runner size, cache hit, arch,
//!    branch, time of day, concurrency, image digest), which move together with failure or with
//!    duration? [`pearson`] for linear strength, [`spearman`] when the relationship is monotone
//!    but not linear (duration versus concurrency usually is).
//! 2. **Attribution** — holding the others fixed, how much does each contribute? [`ols`] with per
//!    coefficient standard errors and t-statistics, so "cache miss costs 94 s ± 11 s" is a
//!    sentence we can defend rather than a slope we eyeballed.
//! 3. **Regression detection** — did a specific commit make it worse? [`welch_t`] compares before
//!    and after without assuming equal variance (CI timings never have equal variance), and
//!    [`cusum_changepoint`] finds *where* a series shifted when nobody told us to look.
//!
//! The part that keeps this honest is [`benjamini_hochberg`]. Scan fifty features at p < 0.05 and
//! roughly two and a half will look significant on pure noise; ranking by p-value and calling the
//! top few "discoveries" is how a dashboard becomes a rumour mill. [`discover`] controls the false
//! discovery rate instead, and reports how many candidates it looked at so the reader can judge.
//!
//! Every function is pure and total: no panics on empty or degenerate input, `None`/`NaN`-free
//! results, and a [`StatsError`] when the data cannot support the question.

use core::cmp::Ordering;
use core::fmt;

/// Why a statistic could not be computed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatsError {
    /// Fewer observations than the method needs.
    TooFewObservations { got: usize, need: usize },
    /// Inputs of different lengths.
    LengthMismatch { left: usize, right: usize },
    /// A column (or the response) never varies, so no relationship is measurable.
    ZeroVariance { column: usize },
    /// More parameters than observations, or perfectly collinear predictors.
    Singular,
    /// A non-finite input; refused rather than propagated.
    NotFinite { index: usize },
}

impl fmt::Display for StatsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatsError::TooFewObservations { got, need } => {
                write!(f, "{got} observations; at least {need} are needed")
            }
            StatsError::LengthMismatch { left, right } => {
                write!(f, "length mismatch: {left} vs {right}")
            }
            StatsError::ZeroVariance { column } => {
                write!(
                    f,
                    "column {column} never varies, so no relationship is measurable"
                )
            }
            StatsError::Singular => f.write_str("design matrix is singular (collinear predictors)"),
            StatsError::NotFinite { index } => write!(f, "observation {index} is not finite"),
        }
    }
}

fn check_finite(values: &[f64]) -> Result<(), StatsError> {
    match values.iter().position(|v| !v.is_finite()) {
        Some(index) => Err(StatsError::NotFinite { index }),
        None => Ok(()),
    }
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

// ---------------------------------------------------------------------------------------------
// Correlation
// ---------------------------------------------------------------------------------------------

/// A correlation and its two-sided p-value under the null of no association.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Correlation {
    pub r: f64,
    /// Two-sided p-value from the t-statistic `r·√((n−2)/(1−r²))` on `n−2` degrees of freedom.
    pub p_value: f64,
    pub observations: usize,
}

impl Correlation {
    /// Coefficient of determination.
    #[must_use]
    pub fn r_squared(self) -> f64 {
        self.r * self.r
    }
}

/// Pearson product-moment correlation.
///
/// # Errors
/// [`StatsError::LengthMismatch`], [`StatsError::TooFewObservations`], [`StatsError::ZeroVariance`],
/// [`StatsError::NotFinite`].
pub fn pearson(x: &[f64], y: &[f64]) -> Result<Correlation, StatsError> {
    if x.len() != y.len() {
        return Err(StatsError::LengthMismatch {
            left: x.len(),
            right: y.len(),
        });
    }
    if x.len() < 3 {
        return Err(StatsError::TooFewObservations {
            got: x.len(),
            need: 3,
        });
    }
    check_finite(x)?;
    check_finite(y)?;

    let (mx, my) = (mean(x), mean(y));
    let mut sxy = 0.0;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    for (xi, yi) in x.iter().zip(y) {
        let (dx, dy) = (xi - mx, yi - my);
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    if sxx == 0.0 {
        return Err(StatsError::ZeroVariance { column: 0 });
    }
    if syy == 0.0 {
        return Err(StatsError::ZeroVariance { column: 1 });
    }
    // Clamp: accumulated rounding can push a perfect correlation a hair outside [-1, 1], and a
    // downstream √(1−r²) would then produce NaN.
    let r = (sxy / (sxx.sqrt() * syy.sqrt())).clamp(-1.0, 1.0);
    let n = x.len() as f64;
    let p_value = if r.abs() >= 1.0 {
        0.0
    } else {
        let t = r * ((n - 2.0) / (1.0 - r * r)).sqrt();
        two_sided_t_p_value(t, n - 2.0)
    };
    Ok(Correlation {
        r,
        p_value,
        observations: x.len(),
    })
}

/// Spearman rank correlation: Pearson on the ranks, with ties given their average rank.
///
/// # Errors
/// As [`pearson`].
pub fn spearman(x: &[f64], y: &[f64]) -> Result<Correlation, StatsError> {
    if x.len() != y.len() {
        return Err(StatsError::LengthMismatch {
            left: x.len(),
            right: y.len(),
        });
    }
    check_finite(x)?;
    check_finite(y)?;
    pearson(&ranks(x), &ranks(y))
}

/// Average ranks, 1-based, ties averaged.
fn ranks(values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|a, b| {
        values[*a]
            .partial_cmp(&values[*b])
            .unwrap_or(Ordering::Equal)
    });
    let mut out = vec![0.0; values.len()];
    let mut i = 0;
    while i < order.len() {
        let mut j = i;
        while j + 1 < order.len() && values[order[j + 1]] == values[order[i]] {
            j += 1;
        }
        // Ranks i..=j are tied; each gets the average of their 1-based positions.
        let average = ((i + 1 + j + 1) as f64) / 2.0;
        for slot in &order[i..=j] {
            out[*slot] = average;
        }
        i = j + 1;
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Ordinary least squares
// ---------------------------------------------------------------------------------------------

/// A fitted linear model, including what is needed to judge it.
#[derive(Clone, Debug, PartialEq)]
pub struct OlsFit {
    /// `coefficients[0]` is the intercept; the rest follow the column order of the design matrix.
    pub coefficients: Vec<f64>,
    pub standard_errors: Vec<f64>,
    /// `coefficient / standard_error`.
    pub t_statistics: Vec<f64>,
    /// Two-sided p-value per coefficient.
    pub p_values: Vec<f64>,
    pub r_squared: f64,
    /// Adjusted for the number of predictors — the one to quote when comparing models.
    pub adjusted_r_squared: f64,
    /// Residual standard error, in the units of the response.
    pub residual_standard_error: f64,
    pub observations: usize,
    pub degrees_of_freedom: f64,
}

/// Fit `y ~ 1 + X` by ordinary least squares, solving the normal equations with Gauss-Jordan
/// elimination and partial pivoting.
///
/// `rows` is row-major: one slice of predictors per observation. An intercept is added; do not
/// include a constant column yourself, or the design matrix is singular.
///
/// # Errors
/// [`StatsError::LengthMismatch`], [`StatsError::TooFewObservations`], [`StatsError::Singular`],
/// [`StatsError::NotFinite`].
pub fn ols(rows: &[Vec<f64>], y: &[f64]) -> Result<OlsFit, StatsError> {
    if rows.len() != y.len() {
        return Err(StatsError::LengthMismatch {
            left: rows.len(),
            right: y.len(),
        });
    }
    check_finite(y)?;
    let predictors = rows.first().map_or(0, Vec::len);
    for row in rows {
        if row.len() != predictors {
            return Err(StatsError::LengthMismatch {
                left: predictors,
                right: row.len(),
            });
        }
        check_finite(row)?;
    }
    let parameters = predictors + 1;
    // Need at least one residual degree of freedom, or the standard errors are undefined.
    if rows.len() <= parameters {
        return Err(StatsError::TooFewObservations {
            got: rows.len(),
            need: parameters + 1,
        });
    }

    // Design matrix with a leading 1 for the intercept.
    let design: Vec<Vec<f64>> = rows
        .iter()
        .map(|row| {
            let mut full = Vec::with_capacity(parameters);
            full.push(1.0);
            full.extend_from_slice(row);
            full
        })
        .collect();

    // XᵀX and Xᵀy
    let mut xtx = vec![vec![0.0; parameters]; parameters];
    let mut xty = vec![0.0; parameters];
    for (row, yi) in design.iter().zip(y) {
        for a in 0..parameters {
            xty[a] += row[a] * yi;
            for b in 0..parameters {
                xtx[a][b] += row[a] * row[b];
            }
        }
    }

    let inverse = invert(&xtx).ok_or(StatsError::Singular)?;
    let coefficients: Vec<f64> = (0..parameters)
        .map(|a| (0..parameters).map(|b| inverse[a][b] * xty[b]).sum())
        .collect();

    let fitted: Vec<f64> = design
        .iter()
        .map(|row| row.iter().zip(&coefficients).map(|(x, c)| x * c).sum())
        .collect();
    let residual_sum_squares: f64 = y
        .iter()
        .zip(&fitted)
        .map(|(yi, fi)| (yi - fi) * (yi - fi))
        .sum();
    let my = mean(y);
    let total_sum_squares: f64 = y.iter().map(|yi| (yi - my) * (yi - my)).sum();

    let degrees_of_freedom = (rows.len() - parameters) as f64;
    let variance = residual_sum_squares / degrees_of_freedom;
    let residual_standard_error = variance.sqrt();

    let standard_errors: Vec<f64> = (0..parameters)
        .map(|a| (variance * inverse[a][a]).max(0.0).sqrt())
        .collect();
    let t_statistics: Vec<f64> = coefficients
        .iter()
        .zip(&standard_errors)
        .map(|(c, se)| if *se > 0.0 { c / se } else { 0.0 })
        .collect();
    let p_values: Vec<f64> = t_statistics
        .iter()
        .map(|t| two_sided_t_p_value(*t, degrees_of_freedom))
        .collect();

    let r_squared = if total_sum_squares > 0.0 {
        1.0 - residual_sum_squares / total_sum_squares
    } else {
        0.0
    };
    let adjusted_r_squared = if total_sum_squares > 0.0 {
        1.0 - (1.0 - r_squared) * ((rows.len() - 1) as f64 / degrees_of_freedom)
    } else {
        0.0
    };

    Ok(OlsFit {
        coefficients,
        standard_errors,
        t_statistics,
        p_values,
        r_squared,
        adjusted_r_squared,
        residual_standard_error,
        observations: rows.len(),
        degrees_of_freedom,
    })
}

/// Gauss-Jordan inversion with partial pivoting. `None` when singular.
fn invert(matrix: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = matrix.len();
    let mut a: Vec<Vec<f64>> = matrix.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();

    for column in 0..n {
        let pivot = (column..n).max_by(|a1, a2| {
            a[*a1][column]
                .abs()
                .partial_cmp(&a[*a2][column].abs())
                .unwrap_or(Ordering::Equal)
        })?;
        if a[pivot][column].abs() < 1e-12 {
            return None;
        }
        a.swap(column, pivot);
        inv.swap(column, pivot);

        let divisor = a[column][column];
        for value in &mut a[column] {
            *value /= divisor;
        }
        for value in &mut inv[column] {
            *value /= divisor;
        }

        for row in 0..n {
            if row == column {
                continue;
            }
            let factor = a[row][column];
            if factor == 0.0 {
                continue;
            }
            for k in 0..n {
                a[row][k] -= factor * a[column][k];
                inv[row][k] -= factor * inv[column][k];
            }
        }
    }
    Some(inv)
}

// ---------------------------------------------------------------------------------------------
// Two-sample comparison
// ---------------------------------------------------------------------------------------------

/// The result of comparing two samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TwoSample {
    pub mean_before: f64,
    pub mean_after: f64,
    /// `mean_after − mean_before`, in the units of the measurement.
    pub difference: f64,
    pub t_statistic: f64,
    /// Welch–Satterthwaite degrees of freedom (fractional on purpose).
    pub degrees_of_freedom: f64,
    pub p_value: f64,
    /// Standardized effect size (Cohen's d on the pooled standard deviation). A significant
    /// p-value with a tiny d means "real but nobody will notice", which is worth saying.
    pub effect_size: f64,
}

/// Welch's unequal-variance t-test — "did this change make it slower?"
///
/// Student's t assumes equal variances; CI timings do not have them (a cache miss has a different
/// spread from a cache hit), and using the pooled version anyway inflates significance.
///
/// # Errors
/// [`StatsError::TooFewObservations`], [`StatsError::ZeroVariance`], [`StatsError::NotFinite`].
pub fn welch_t(before: &[f64], after: &[f64]) -> Result<TwoSample, StatsError> {
    if before.len() < 2 {
        return Err(StatsError::TooFewObservations {
            got: before.len(),
            need: 2,
        });
    }
    if after.len() < 2 {
        return Err(StatsError::TooFewObservations {
            got: after.len(),
            need: 2,
        });
    }
    check_finite(before)?;
    check_finite(after)?;

    let (n1, n2) = (before.len() as f64, after.len() as f64);
    let (m1, m2) = (mean(before), mean(after));
    let v1 = before.iter().map(|v| (v - m1) * (v - m1)).sum::<f64>() / (n1 - 1.0);
    let v2 = after.iter().map(|v| (v - m2) * (v - m2)).sum::<f64>() / (n2 - 1.0);

    if v1 == 0.0 && v2 == 0.0 {
        return Err(StatsError::ZeroVariance { column: 0 });
    }

    let standard_error = (v1 / n1 + v2 / n2).sqrt();
    let t = (m2 - m1) / standard_error;
    let numerator = (v1 / n1 + v2 / n2).powi(2);
    let denominator = (v1 / n1).powi(2) / (n1 - 1.0) + (v2 / n2).powi(2) / (n2 - 1.0);
    let df = if denominator > 0.0 {
        numerator / denominator
    } else {
        n1 + n2 - 2.0
    };
    let pooled = (((n1 - 1.0) * v1 + (n2 - 1.0) * v2) / (n1 + n2 - 2.0)).sqrt();

    Ok(TwoSample {
        mean_before: m1,
        mean_after: m2,
        difference: m2 - m1,
        t_statistic: t,
        degrees_of_freedom: df,
        p_value: two_sided_t_p_value(t, df),
        effect_size: if pooled > 0.0 {
            (m2 - m1) / pooled
        } else {
            0.0
        },
    })
}

// ---------------------------------------------------------------------------------------------
// Change points
// ---------------------------------------------------------------------------------------------

/// Where a series shifted, and by how much.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChangePoint {
    /// Index of the first observation *after* the shift.
    pub index: usize,
    pub mean_before: f64,
    pub mean_after: f64,
    pub difference: f64,
    /// p-value of the Welch test across the split. Sharpened by the search, so treat it as a
    /// ranking score, not a calibrated probability — the search itself is a multiple comparison.
    pub p_value: f64,
}

/// Find the single most likely level shift in a series, by maximizing the CUSUM statistic and
/// then testing the implied split.
///
/// `min_segment` guards both ends: a "regression" inferred from two observations is noise.
/// Returns `None` when nothing crosses `alpha`.
///
/// # Errors
/// [`StatsError::TooFewObservations`], [`StatsError::NotFinite`].
pub fn cusum_changepoint(
    series: &[f64],
    min_segment: usize,
    alpha: f64,
) -> Result<Option<ChangePoint>, StatsError> {
    let need = min_segment.max(2) * 2;
    if series.len() < need {
        return Err(StatsError::TooFewObservations {
            got: series.len(),
            need,
        });
    }
    check_finite(series)?;

    let overall = mean(series);
    // Cumulative sum of deviations; its extremum is the maximum-likelihood split for a single
    // level shift in a Gaussian series.
    let mut cumulative = 0.0;
    let mut best_index = min_segment.max(2);
    let mut best_score = f64::NEG_INFINITY;
    for (index, value) in series.iter().enumerate() {
        cumulative += value - overall;
        let split = index + 1;
        if split < min_segment.max(2) || series.len() - split < min_segment.max(2) {
            continue;
        }
        // Normalize so a split near either end is not favoured purely by having more terms.
        let n = series.len() as f64;
        let s = split as f64;
        let score = cumulative.abs() / (s * (n - s) / n).sqrt();
        if score > best_score {
            best_score = score;
            best_index = split;
        }
    }

    let (before, after) = series.split_at(best_index);
    let test = match welch_t(before, after) {
        Ok(test) => test,
        Err(StatsError::ZeroVariance { .. }) => {
            // Both halves constant: a real shift if the levels differ at all.
            let (m1, m2) = (mean(before), mean(after));
            if (m2 - m1).abs() > 0.0 {
                return Ok(Some(ChangePoint {
                    index: best_index,
                    mean_before: m1,
                    mean_after: m2,
                    difference: m2 - m1,
                    p_value: 0.0,
                }));
            }
            return Ok(None);
        }
        Err(other) => return Err(other),
    };

    Ok((test.p_value <= alpha).then_some(ChangePoint {
        index: best_index,
        mean_before: test.mean_before,
        mean_after: test.mean_after,
        difference: test.difference,
        p_value: test.p_value,
    }))
}

// ---------------------------------------------------------------------------------------------
// Multiple testing and discovery
// ---------------------------------------------------------------------------------------------

/// Benjamini-Hochberg step-up: returns, for each input p-value **in the original order**, the
/// adjusted value (q-value), monotone-corrected.
///
/// Controls the expected proportion of false positives among the things you call significant —
/// the right guarantee when scanning many candidate features, where Bonferroni would be so
/// conservative that nothing is ever reported.
#[must_use]
pub fn benjamini_hochberg(p_values: &[f64]) -> Vec<f64> {
    let m = p_values.len();
    if m == 0 {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..m).collect();
    order.sort_by(|a, b| {
        p_values[*a]
            .partial_cmp(&p_values[*b])
            .unwrap_or(Ordering::Equal)
    });

    let mut adjusted = vec![0.0; m];
    let mut running = f64::INFINITY;
    // Walk from the largest p downwards, enforcing monotonicity.
    for rank in (0..m).rev() {
        let index = order[rank];
        let candidate = p_values[index] * (m as f64) / ((rank + 1) as f64);
        running = running.min(candidate).min(1.0);
        adjusted[index] = running;
    }
    adjusted
}

/// One candidate relationship, after multiple-testing correction.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub feature: String,
    pub correlation: Correlation,
    /// Benjamini-Hochberg adjusted p-value across every candidate examined.
    pub q_value: f64,
    pub significant: bool,
}

/// The result of a discovery scan, including the denominator.
#[derive(Clone, Debug, PartialEq)]
pub struct Discovery {
    /// Significant findings, strongest correlation first.
    pub findings: Vec<Finding>,
    /// How many candidates were examined — without this number, a list of "significant" features
    /// cannot be interpreted.
    pub candidates_examined: usize,
    /// Candidates skipped because they were constant or too short, with the reason.
    pub skipped: Vec<(String, StatsError)>,
    pub false_discovery_rate: f64,
}

/// Scan candidate features for association with `response`, controlling the false discovery rate.
///
/// `features` is `(name, values)`. A feature that cannot be tested (constant, too short, contains
/// a non-finite value) is *reported as skipped* rather than dropped: "we could not test this" and
/// "we tested this and found nothing" are different statements.
///
/// `monotone` selects [`spearman`] over [`pearson`] — the right default for durations and counts,
/// where the relationship is reliably monotone and only sometimes linear.
#[must_use]
pub fn discover(
    features: &[(String, Vec<f64>)],
    response: &[f64],
    false_discovery_rate: f64,
    monotone: bool,
) -> Discovery {
    let mut names = Vec::new();
    let mut correlations = Vec::new();
    let mut skipped = Vec::new();

    for (name, values) in features {
        let result = if monotone {
            spearman(values, response)
        } else {
            pearson(values, response)
        };
        match result {
            Ok(correlation) => {
                names.push(name.clone());
                correlations.push(correlation);
            }
            Err(error) => skipped.push((name.clone(), error)),
        }
    }

    let p_values: Vec<f64> = correlations.iter().map(|c| c.p_value).collect();
    let q_values = benjamini_hochberg(&p_values);

    let mut findings: Vec<Finding> = names
        .into_iter()
        .zip(correlations)
        .zip(q_values)
        .map(|((feature, correlation), q_value)| Finding {
            feature,
            correlation,
            q_value,
            significant: q_value <= false_discovery_rate,
        })
        .filter(|finding| finding.significant)
        .collect();
    findings.sort_by(|a, b| {
        b.correlation
            .r
            .abs()
            .partial_cmp(&a.correlation.r.abs())
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.feature.cmp(&b.feature))
    });

    Discovery {
        findings,
        candidates_examined: features.len(),
        skipped,
        false_discovery_rate,
    }
}

// ---------------------------------------------------------------------------------------------
// Distributions
// ---------------------------------------------------------------------------------------------

/// Two-sided p-value for a t-statistic on `df` degrees of freedom.
///
/// Computed from the regularized incomplete beta function, which is exact for the t distribution
/// rather than the normal approximation that goes visibly wrong below about 30 df — precisely the
/// sample sizes a CI regression check runs on.
#[must_use]
pub fn two_sided_t_p_value(t: f64, df: f64) -> f64 {
    if !t.is_finite() || df <= 0.0 {
        return 1.0;
    }
    let x = df / (df + t * t);
    incomplete_beta(df / 2.0, 0.5, x).clamp(0.0, 1.0)
}

/// Regularized incomplete beta function `I_x(a, b)`, by the continued fraction with the standard
/// symmetry reflection for fast convergence.
#[must_use]
pub fn incomplete_beta(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let front =
        (ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln()).exp();
    // The continued fraction converges quickly only on one side of the mode; reflect otherwise.
    if x < (a + 1.0) / (a + b + 2.0) {
        front * beta_continued_fraction(a, b, x) / a
    } else {
        1.0 - front * beta_continued_fraction(b, a, 1.0 - x) / b
    }
}

fn beta_continued_fraction(a: f64, b: f64, x: f64) -> f64 {
    const MAX_ITERATIONS: usize = 300;
    const EPSILON: f64 = 1e-15;
    const TINY: f64 = 1e-300;

    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < TINY {
        d = TINY;
    }
    d = 1.0 / d;
    let mut h = d;

    for m in 1..=MAX_ITERATIONS {
        let m_f = m as f64;
        let m2 = 2.0 * m_f;

        // Even step
        let numerator = m_f * (b - m_f) * x / ((qam + m2) * (a + m2));
        d = 1.0 + numerator * d;
        if d.abs() < TINY {
            d = TINY;
        }
        c = 1.0 + numerator / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        h *= d * c;

        // Odd step
        let numerator = -(a + m_f) * (qab + m_f) * x / ((a + m2) * (qap + m2));
        d = 1.0 + numerator * d;
        if d.abs() < TINY {
            d = TINY;
        }
        c = 1.0 + numerator / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;

        if (delta - 1.0).abs() < EPSILON {
            break;
        }
    }
    h
}

/// Lanczos approximation to `ln Γ(x)` for `x > 0`.
#[must_use]
pub fn ln_gamma(x: f64) -> f64 {
    const COEFFICIENTS: [f64; 6] = [
        76.180_091_729_471_46,
        -86.505_320_329_416_77,
        24.014_098_240_830_91,
        -1.231_739_572_450_155,
        0.120_865_097_386_617_7e-2,
        -0.539_523_938_495_3e-5,
    ];
    let mut y = x;
    let tmp = x + 5.5;
    let tmp = tmp - (x + 0.5) * tmp.ln();
    let mut series = 1.000_000_000_190_015;
    for coefficient in COEFFICIENTS {
        y += 1.0;
        series += coefficient / y;
    }
    -tmp + (2.506_628_274_631_000_5 * series / x).ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn a_perfect_line_correlates_exactly() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y: Vec<f64> = x.iter().map(|v| 3.0 * v + 7.0).collect();
        let c = pearson(&x, &y).unwrap();
        assert!(close(c.r, 1.0, 1e-12));
        assert!(close(c.p_value, 0.0, 1e-12));

        let inverted: Vec<f64> = x.iter().map(|v| -3.0 * v).collect();
        assert!(close(pearson(&x, &inverted).unwrap().r, -1.0, 1e-12));
    }

    #[test]
    fn ols_recovers_the_generating_coefficients_exactly() {
        // y = 5 + 2·x1 − 3·x2, noiseless.
        let rows: Vec<Vec<f64>> = vec![
            vec![1.0, 0.0],
            vec![2.0, 1.0],
            vec![3.0, 0.0],
            vec![4.0, 2.0],
            vec![5.0, 1.0],
            vec![6.0, 3.0],
        ];
        let y: Vec<f64> = rows.iter().map(|r| 5.0 + 2.0 * r[0] - 3.0 * r[1]).collect();
        let fit = ols(&rows, &y).unwrap();
        assert!(
            close(fit.coefficients[0], 5.0, 1e-9),
            "intercept {}",
            fit.coefficients[0]
        );
        assert!(
            close(fit.coefficients[1], 2.0, 1e-9),
            "x1 {}",
            fit.coefficients[1]
        );
        assert!(
            close(fit.coefficients[2], -3.0, 1e-9),
            "x2 {}",
            fit.coefficients[2]
        );
        assert!(close(fit.r_squared, 1.0, 1e-9));
        assert!(fit.residual_standard_error < 1e-9);
    }

    #[test]
    fn collinear_predictors_are_refused_rather_than_returning_nonsense() {
        let rows: Vec<Vec<f64>> = (1..=8).map(|i| vec![i as f64, 2.0 * i as f64]).collect();
        let y: Vec<f64> = (1..=8).map(|i| i as f64).collect();
        assert_eq!(ols(&rows, &y).unwrap_err(), StatsError::Singular);
    }

    #[test]
    fn too_few_observations_for_the_parameters_is_refused() {
        let rows: Vec<Vec<f64>> = vec![vec![1.0, 1.0], vec![2.0, 4.0], vec![3.0, 9.0]];
        let y = vec![1.0, 2.0, 3.0];
        assert_eq!(
            ols(&rows, &y).unwrap_err(),
            StatsError::TooFewObservations { got: 3, need: 4 }
        );
    }

    #[test]
    fn spearman_sees_a_monotone_curve_that_pearson_underrates() {
        let x: Vec<f64> = (1..=12).map(f64::from).collect();
        let y: Vec<f64> = x.iter().map(|v| v.powi(4)).collect();
        let s = spearman(&x, &y).unwrap();
        let p = pearson(&x, &y).unwrap();
        assert!(close(s.r, 1.0, 1e-12), "spearman {}", s.r);
        assert!(
            p.r < s.r,
            "pearson {} should be below spearman {}",
            p.r,
            s.r
        );
    }

    #[test]
    fn spearman_handles_ties_with_average_ranks() {
        assert_eq!(ranks(&[10.0, 20.0, 20.0, 30.0]), vec![1.0, 2.5, 2.5, 4.0]);
        assert_eq!(ranks(&[5.0, 5.0, 5.0]), vec![2.0, 2.0, 2.0]);
    }

    #[test]
    fn welch_detects_a_real_slowdown_and_ignores_noise() {
        let before = vec![100.0, 102.0, 98.0, 101.0, 99.0, 100.5, 101.5, 99.5];
        let after = vec![140.0, 145.0, 138.0, 142.0, 139.0, 144.0, 141.0, 143.0];
        let test = welch_t(&before, &after).unwrap();
        assert!(
            test.difference > 39.0 && test.difference < 43.0,
            "{}",
            test.difference
        );
        assert!(test.p_value < 1e-6, "p {}", test.p_value);
        assert!(test.effect_size > 2.0, "d {}", test.effect_size);

        // Same distribution: no finding.
        let a = vec![100.0, 102.0, 98.0, 101.0, 99.0, 100.5];
        let b = vec![101.0, 99.0, 100.0, 102.0, 98.5, 100.2];
        assert!(welch_t(&a, &b).unwrap().p_value > 0.2);
    }

    #[test]
    fn a_step_change_is_located_at_the_right_index() {
        let mut series: Vec<f64> = vec![100.0, 101.0, 99.0, 100.5, 100.0, 99.5, 101.0, 100.0];
        series.extend([150.0, 151.0, 149.0, 150.5, 150.0, 149.5, 151.0, 150.0]);
        let found = cusum_changepoint(&series, 3, 0.01)
            .unwrap()
            .expect("a shift this large is found");
        assert_eq!(found.index, 8, "shift is between index 7 and 8");
        assert!(close(found.difference, 50.0, 1.0), "{}", found.difference);
    }

    #[test]
    fn a_flat_series_yields_no_change_point() {
        let series: Vec<f64> = (0..20).map(|i| 100.0 + ((i % 3) as f64) * 0.1).collect();
        assert_eq!(cusum_changepoint(&series, 4, 0.01).unwrap(), None);
    }

    #[test]
    fn benjamini_hochberg_is_monotone_and_leaves_a_single_p_value_alone() {
        assert_eq!(benjamini_hochberg(&[0.04]), vec![0.04]);
        let adjusted = benjamini_hochberg(&[0.001, 0.008, 0.039, 0.041, 0.042, 0.06, 0.074, 0.205]);
        for pair in adjusted.windows(2) {
            assert!(pair[0] <= pair[1] + 1e-12, "not monotone: {pair:?}");
        }
        assert!(adjusted.iter().all(|q| *q <= 1.0));
        assert!(close(adjusted[0], 0.008, 1e-9), "{}", adjusted[0]);
    }

    #[test]
    fn discovery_reports_the_denominator_and_what_it_could_not_test() {
        let response: Vec<f64> = (1..=20).map(f64::from).collect();
        let signal: Vec<f64> = response.iter().map(|v| v * 2.0 + 1.0).collect();
        let noise = vec![
            3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0, 5.0, 3.0, 5.0, 8.0, 9.0, 7.0, 9.0, 3.0, 2.0,
            3.0, 8.0, 4.0,
        ];
        let constant = vec![7.0; 20];
        let short = vec![1.0, 2.0];

        let discovery = discover(
            &[
                ("cache_miss_seconds".into(), signal),
                ("moon_phase".into(), noise),
                ("always_seven".into(), constant),
                ("too_short".into(), short),
            ],
            &response,
            0.05,
            true,
        );

        assert_eq!(discovery.candidates_examined, 4);
        assert_eq!(discovery.findings.len(), 1);
        assert_eq!(discovery.findings[0].feature, "cache_miss_seconds");
        assert_eq!(
            discovery.skipped.len(),
            2,
            "constant and short features are reported, not dropped"
        );
        assert!(discovery
            .skipped
            .iter()
            .any(|(name, _)| name == "always_seven"));
        assert!(discovery
            .skipped
            .iter()
            .any(|(name, _)| name == "too_short"));
    }

    #[test]
    fn degenerate_and_hostile_inputs_never_panic() {
        assert!(matches!(
            pearson(&[], &[]),
            Err(StatsError::TooFewObservations { .. })
        ));
        assert!(matches!(
            pearson(&[1.0, 2.0], &[1.0]),
            Err(StatsError::LengthMismatch { .. })
        ));
        assert!(matches!(
            pearson(&[1.0, 1.0, 1.0], &[1.0, 2.0, 3.0]),
            Err(StatsError::ZeroVariance { column: 0 })
        ));
        assert!(matches!(
            pearson(&[1.0, f64::NAN, 3.0], &[1.0, 2.0, 3.0]),
            Err(StatsError::NotFinite { index: 1 })
        ));
        assert!(matches!(
            welch_t(&[1.0], &[1.0, 2.0]),
            Err(StatsError::TooFewObservations { .. })
        ));
        assert_eq!(benjamini_hochberg(&[]), Vec::<f64>::new());
        assert_eq!(two_sided_t_p_value(f64::NAN, 5.0), 1.0);
        assert_eq!(two_sided_t_p_value(2.0, 0.0), 1.0);
        // Every p-value stays a probability.
        for t in [-1e300, -10.0, 0.0, 10.0, 1e300] {
            for df in [1.0, 2.5, 30.0, 1e6] {
                let p = two_sided_t_p_value(t, df);
                assert!((0.0..=1.0).contains(&p), "t={t} df={df} p={p}");
            }
        }
    }
}
