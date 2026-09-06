// Emits every statistic the Python oracle re-checks. Run from the repo root:
//   rustc --edition 2021 -O oracle/xcheck.rs -o /tmp/xcheck && /tmp/xcheck > /tmp/rust_out.txt
//   python3 oracle/crosscheck.py /tmp/rust_out.txt
#[path = "../src/analysis/regression.rs"]
mod regression;
use regression::*;


fn main() {
    // 1. t-distribution two-sided p-values across the range that matters (small df especially).
    println!("# t_pvalues");
    for df in [1.0_f64, 2.0, 3.0, 5.0, 8.0, 10.0, 20.0, 30.0, 60.0, 120.0, 1000.0, 7.3] {
        for t in [0.0_f64, 0.25, 0.7, 1.0, 1.6449, 1.96, 2.0, 2.228, 2.5, 3.0, 4.5, 8.0, -2.086] {
            println!("t {df} {t} {:.15e}", two_sided_t_p_value(t, df));
        }
    }

    // 2. Pearson r and p.
    println!("# pearson");
    let xs: Vec<f64> = vec![1.0, 2.0, 3.5, 4.0, 5.9, 6.1, 7.0, 8.8, 9.0, 10.2];
    let ys: Vec<f64> = vec![2.1, 3.9, 6.2, 8.5, 11.0, 12.4, 13.1, 17.9, 18.2, 21.0];
    let c = pearson(&xs, &ys).unwrap();
    println!("pearson {:.15e} {:.15e}", c.r, c.p_value);
    let zs: Vec<f64> = vec![9.0, 1.0, 8.0, 2.0, 7.0, 3.0, 6.0, 4.0, 5.0, 0.5];
    let c2 = pearson(&xs, &zs).unwrap();
    println!("pearson2 {:.15e} {:.15e}", c2.r, c2.p_value);

    // 3. Spearman with ties.
    println!("# spearman");
    let a: Vec<f64> = vec![1.0, 2.0, 2.0, 4.0, 5.0, 6.0, 6.0, 6.0, 9.0, 10.0];
    let b: Vec<f64> = vec![3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0, 5.0, 3.0];
    let s = spearman(&a, &b).unwrap();
    println!("spearman {:.15e} {:.15e}", s.r, s.p_value);

    // 4. Welch t-test.
    println!("# welch");
    let before = vec![100.0, 102.0, 98.0, 101.0, 99.0, 100.5, 101.5, 99.5];
    let after = vec![140.0, 145.0, 138.0, 142.0, 139.0, 144.0, 141.0, 143.0, 160.0];
    let w = welch_t(&before, &after).unwrap();
    println!("welch {:.15e} {:.15e} {:.15e}", w.t_statistic, w.degrees_of_freedom, w.p_value);

    // 5. OLS.
    println!("# ols");
    let rows: Vec<Vec<f64>> = vec![
        vec![1.0, 3.0], vec![2.0, 1.0], vec![3.0, 4.0], vec![4.0, 1.0], vec![5.0, 5.0],
        vec![6.0, 9.0], vec![7.0, 2.0], vec![8.0, 6.0], vec![9.0, 5.0], vec![10.0, 3.0],
        vec![11.0, 5.0], vec![12.0, 8.0],
    ];
    let y: Vec<f64> = vec![12.1, 9.4, 17.2, 12.0, 22.5, 31.0, 18.2, 28.4, 27.1, 24.0, 29.9, 36.2];
    let fit = ols(&rows, &y).unwrap();
    for (i, c) in fit.coefficients.iter().enumerate() {
        println!("ols_coef {i} {:.15e} {:.15e} {:.15e} {:.15e}", c, fit.standard_errors[i], fit.t_statistics[i], fit.p_values[i]);
    }
    println!("ols_r2 {:.15e} {:.15e} {:.15e}", fit.r_squared, fit.adjusted_r_squared, fit.residual_standard_error);

    // 6. Benjamini-Hochberg.
    println!("# bh");
    let ps = vec![0.001, 0.008, 0.039, 0.041, 0.042, 0.06, 0.074, 0.205, 0.212, 0.216, 0.222, 0.251, 0.269, 0.275, 0.34, 0.641, 0.679, 0.869, 0.999, 0.0001];
    for (i, q) in benjamini_hochberg(&ps).iter().enumerate() {
        println!("bh {i} {:.15e}", q);
    }

    // 7. ln_gamma.
    println!("# lngamma");
    for x in [0.5_f64, 1.0, 1.5, 2.0, 3.7, 10.0, 50.0, 0.001, 120.5] {
        println!("lngamma {x} {:.15e}", ln_gamma(x));
    }
}
