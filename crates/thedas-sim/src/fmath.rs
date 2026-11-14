//! Math functions that give the same results on every platform. The simulation must be
//! deterministic (the regression gate and journal-style saves replay it), and `f32::sin`,
//! `cos`, `atan2`, `powf`, `exp` and `ln` call the platform's math library, whose last bits
//! differ between macOS, Windows and Linux. These use the `libm` crate (a pure Rust port of
//! musl's) instead. `sqrt`, `abs`, `floor` and the like are exact everywhere and stay as
//! they are.

/// Platform-independent versions of `f32`'s transcendental functions, named with a `d`
/// (deterministic) in front.
pub trait DetMath: Copy {
    fn dsin(self) -> Self;
    fn dcos(self) -> Self;
    fn dsin_cos(self) -> (Self, Self);
    fn dtan(self) -> Self;
    fn dasin(self) -> Self;
    fn dacos(self) -> Self;
    fn datan(self) -> Self;
    fn datan2(self, x: Self) -> Self;
    fn dpowf(self, e: Self) -> Self;
    fn dexp(self) -> Self;
    fn dln(self) -> Self;
    fn dhypot(self, o: Self) -> Self;
}

impl DetMath for f32 {
    fn dsin(self) -> f32 {
        libm::sinf(self)
    }
    fn dcos(self) -> f32 {
        libm::cosf(self)
    }
    fn dsin_cos(self) -> (f32, f32) {
        (libm::sinf(self), libm::cosf(self))
    }
    fn dtan(self) -> f32 {
        libm::tanf(self)
    }
    fn dasin(self) -> f32 {
        libm::asinf(self)
    }
    fn dacos(self) -> f32 {
        libm::acosf(self)
    }
    fn datan(self) -> f32 {
        libm::atanf(self)
    }
    fn datan2(self, x: f32) -> f32 {
        libm::atan2f(self, x)
    }
    fn dpowf(self, e: f32) -> f32 {
        libm::powf(self, e)
    }
    fn dexp(self) -> f32 {
        libm::expf(self)
    }
    fn dln(self) -> f32 {
        libm::logf(self)
    }
    fn dhypot(self, o: f32) -> f32 {
        libm::hypotf(self, o)
    }
}

impl DetMath for f64 {
    fn dsin(self) -> f64 {
        libm::sin(self)
    }
    fn dcos(self) -> f64 {
        libm::cos(self)
    }
    fn dsin_cos(self) -> (f64, f64) {
        (libm::sin(self), libm::cos(self))
    }
    fn dtan(self) -> f64 {
        libm::tan(self)
    }
    fn dasin(self) -> f64 {
        libm::asin(self)
    }
    fn dacos(self) -> f64 {
        libm::acos(self)
    }
    fn datan(self) -> f64 {
        libm::atan(self)
    }
    fn datan2(self, x: f64) -> f64 {
        libm::atan2(self, x)
    }
    fn dpowf(self, e: f64) -> f64 {
        libm::pow(self, e)
    }
    fn dexp(self) -> f64 {
        libm::exp(self)
    }
    fn dln(self) -> f64 {
        libm::log(self)
    }
    fn dhypot(self, o: f64) -> f64 {
        libm::hypot(self, o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_deterministic_functions_agree_with_the_platforms_closely() {
        for i in -40..40 {
            let x = i as f32 * 0.137;
            assert!((x.dsin() - x.sin()).abs() < 1e-6);
            assert!((x.dcos() - x.cos()).abs() < 1e-6);
            assert!((x.datan2(1.3) - x.atan2(1.3)).abs() < 1e-6);
            assert!((x.dexp() - x.exp()).abs() <= 1e-6 * x.exp().max(1.0));
        }
    }
}
