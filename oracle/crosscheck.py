#!/usr/bin/env python3
"""Independent oracle for gha-indie-worker-lib-core's statistics.

scipy/numpy recompute every value the Rust implementation printed. Two implementations that
disagree mean one of them is wrong; agreement to ~1e-10 across p-values, regression coefficients
and FDR adjustment is the evidence that the Rust module is usable for regression detection.

Usage: /tmp/xcheck > rust_out.txt && python3 crosscheck.py rust_out.txt
"""
import sys, math
import numpy as np
from scipy import stats

rows = [l.split() for l in open(sys.argv[1]) if l.strip() and not l.startswith('#')]
checks = failures = 0

def cmp(label, got, want, rel=1e-9, abs_=1e-12):
    global checks, failures
    checks += 1
    if math.isclose(got, want, rel_tol=rel, abs_tol=abs_):
        return
    failures += 1
    print(f"  MISMATCH {label}: rust={got!r} scipy={want!r} (rel diff {abs(got-want)/max(abs(want),1e-300):.3e})")

# --- 1. t-distribution two-sided p-values -----------------------------------------------------
for r in [r for r in rows if r[0] == 't']:
    df, t, got = float(r[1]), float(r[2]), float(r[3])
    cmp(f"t_p(t={t}, df={df})", got, 2 * stats.t.sf(abs(t), df))

# --- 2/3. Pearson and Spearman ----------------------------------------------------------------
xs = [1.0, 2.0, 3.5, 4.0, 5.9, 6.1, 7.0, 8.8, 9.0, 10.2]
ys = [2.1, 3.9, 6.2, 8.5, 11.0, 12.4, 13.1, 17.9, 18.2, 21.0]
zs = [9.0, 1.0, 8.0, 2.0, 7.0, 3.0, 6.0, 4.0, 5.0, 0.5]
for tag, a, b in (("pearson", xs, ys), ("pearson2", xs, zs)):
    row = next(r for r in rows if r[0] == tag)
    ref = stats.pearsonr(a, b)
    cmp(f"{tag}.r", float(row[1]), ref.statistic)
    cmp(f"{tag}.p", float(row[2]), ref.pvalue)

a = [1.0, 2.0, 2.0, 4.0, 5.0, 6.0, 6.0, 6.0, 9.0, 10.0]
b = [3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0, 5.0, 3.0]
row = next(r for r in rows if r[0] == 'spearman')
ref = stats.spearmanr(a, b)
cmp("spearman.r", float(row[1]), ref.statistic)
cmp("spearman.p", float(row[2]), ref.pvalue)

# --- 4. Welch -------------------------------------------------------------------------------
before = [100.0, 102.0, 98.0, 101.0, 99.0, 100.5, 101.5, 99.5]
after = [140.0, 145.0, 138.0, 142.0, 139.0, 144.0, 141.0, 143.0, 160.0]
row = next(r for r in rows if r[0] == 'welch')
ref = stats.ttest_ind(after, before, equal_var=False)
cmp("welch.t", float(row[1]), ref.statistic)
cmp("welch.df", float(row[2]), ref.df)
cmp("welch.p", float(row[3]), ref.pvalue)

# --- 5. OLS ---------------------------------------------------------------------------------
X = np.array([[1.0,3.0],[2.0,1.0],[3.0,4.0],[4.0,1.0],[5.0,5.0],[6.0,9.0],
              [7.0,2.0],[8.0,6.0],[9.0,5.0],[10.0,3.0],[11.0,5.0],[12.0,8.0]])
y = np.array([12.1,9.4,17.2,12.0,22.5,31.0,18.2,28.4,27.1,24.0,29.9,36.2])
D = np.column_stack([np.ones(len(X)), X])
beta, *_ = np.linalg.lstsq(D, y, rcond=None)
resid = y - D @ beta
n, k = D.shape
dof = n - k
sigma2 = resid @ resid / dof
cov = sigma2 * np.linalg.inv(D.T @ D)
se = np.sqrt(np.diag(cov))
tstat = beta / se
pval = 2 * stats.t.sf(np.abs(tstat), dof)
r2 = 1 - (resid @ resid) / (((y - y.mean()) ** 2).sum())
adj = 1 - (1 - r2) * (n - 1) / dof
for r in [r for r in rows if r[0] == 'ols_coef']:
    i = int(r[1])
    cmp(f"ols.coef[{i}]", float(r[2]), beta[i])
    cmp(f"ols.se[{i}]", float(r[3]), se[i])
    cmp(f"ols.t[{i}]", float(r[4]), tstat[i])
    cmp(f"ols.p[{i}]", float(r[5]), pval[i])
row = next(r for r in rows if r[0] == 'ols_r2')
cmp("ols.r2", float(row[1]), r2)
cmp("ols.adj_r2", float(row[2]), adj)
cmp("ols.rse", float(row[3]), math.sqrt(sigma2))

# --- 6. Benjamini-Hochberg -------------------------------------------------------------------
ps = [0.001,0.008,0.039,0.041,0.042,0.06,0.074,0.205,0.212,0.216,
      0.222,0.251,0.269,0.275,0.34,0.641,0.679,0.869,0.999,0.0001]
ref = stats.false_discovery_control(ps, method='bh')
for r in [r for r in rows if r[0] == 'bh']:
    i = int(r[1])
    cmp(f"bh[{i}]", float(r[2]), float(ref[i]))

# --- 7. ln_gamma ------------------------------------------------------------------------------
for r in [r for r in rows if r[0] == 'lngamma']:
    x, got = float(r[1]), float(r[2])
    cmp(f"lngamma({x})", got, math.lgamma(x), rel=1e-12)

print(f"[oracle] {checks} values cross-checked against scipy/numpy, {failures} mismatch(es)")
sys.exit(1 if failures else 0)
