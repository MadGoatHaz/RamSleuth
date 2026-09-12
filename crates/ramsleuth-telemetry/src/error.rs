//! Telemetry error types + no-panic result contract (P2-02, interface freeze).
//!
//! This module freezes the two error/result types every downstream chunk
//! (P2-03 … P2-11) returns:
//!
//! - [`TelemetryError`]: the rich failure carried by [`TelemetryResult<T>`],
//!   keeping the full cause: vendor string, driver name, privilege hint,
//!   PM-table version, the underlying `std::io::Error`, or parse detail.
//! - [`NaReason`]: the light reason tag that is *displayed* in a grid cell
//!   as `N/A (<reason>)`.
//! - [`Section<T>`]: the per-field wrapper (`Value(T) | Na(NaReason)`) and
//!   the crate-wide no-panic contract: any unsupported / privileged /
//!   unknown state degrades to [`Section::Na`] instead of panicking.
//!
//! The `Display` texts are user-presentable and surface verbatim in the
//! P2-11 CLI.
//!
//! **No-panic contract:** nothing in this crate may `panic!`, `unwrap()`, or
//! `expect()` on hardware-derived data (plan decision D5).

/// A rich telemetry failure carried by [`TelemetryResult`].
///
/// Frozen (P2-02, interface freeze): the variants and field shapes are the
/// contract every downstream chunk branches on.
///
/// `std::io::Error` (held by [`TelemetryError::Io`]) is neither `Clone`
/// nor `PartialEq` in the standard library, so those traits — and the `Eq`
/// marker — are implemented manually below instead of derived: `Clone`
/// reconstructs the inner `io::Error` from its raw OS code (or, for custom
/// errors, from kind and text); `PartialEq` compares `io::Error`s by kind
/// and raw OS code. `Eq` holds because that comparison is an equivalence
/// relation.
#[derive(Debug)]
pub enum TelemetryError {
    /// The detected hardware (CPU vendor / microarchitecture) is not
    /// supported by this telemetry source. `vendor` is the vendor string as
    /// reported by CPUID detection (may be an unknown brand string).
    UnsupportedHardware { vendor: String },

    /// A required kernel driver is not loaded (device node / sysfs path
    /// missing). `driver` names the driver or device (e.g. `"ryzen_smu"`).
    DriverMissing { driver: &'static str },

    /// The operation requires more privilege than the caller has. `hint`
    /// tells the user how to resolve it (e.g. `"run as root"`).
    InsufficientPrivilege { hint: &'static str },

    /// The SMU PM-table version is outside the supported layout set.
    /// `version` is the raw PMFW version word read from the PM header.
    UnknownPmTableVersion { version: u32 },

    /// A raw I/O failure (file open/read, ioctl, mmap). The inner
    /// `std::io::Error` is exposed through `std::error::Error::source()`.
    Io(std::io::Error),

    /// A payload read from hardware was malformed. `detail` names the field
    /// or byte range that failed to parse.
    Parse { detail: String },
}

impl Clone for TelemetryError {
    fn clone(&self) -> Self {
        match self {
            Self::UnsupportedHardware { vendor } => Self::UnsupportedHardware {
                vendor: vendor.clone(),
            },
            Self::DriverMissing { driver } => Self::DriverMissing { driver },
            Self::InsufficientPrivilege { hint } => Self::InsufficientPrivilege { hint },
            Self::UnknownPmTableVersion { version } => Self::UnknownPmTableVersion {
                version: *version,
            },
            // `std::io::Error` is not `Clone`: reconstruct from the raw OS
            // code when present (keeps the platform message), otherwise from
            // kind + text.
            Self::Io(err) => match err.raw_os_error() {
                Some(code) => Self::Io(std::io::Error::from_raw_os_error(code)),
                None => Self::Io(std::io::Error::new(err.kind(), err.to_string())),
            },
            Self::Parse { detail } => Self::Parse {
                detail: detail.clone(),
            },
        }
    }
}

impl PartialEq for TelemetryError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::UnsupportedHardware { vendor: a },
                Self::UnsupportedHardware { vendor: b },
            ) => a == b,
            (Self::DriverMissing { driver: a }, Self::DriverMissing { driver: b }) => a == b,
            (
                Self::InsufficientPrivilege { hint: a },
                Self::InsufficientPrivilege { hint: b },
            ) => a == b,
            (
                Self::UnknownPmTableVersion { version: a },
                Self::UnknownPmTableVersion { version: b },
            ) => a == b,
            (Self::Io(a), Self::Io(b)) => {
                a.kind() == b.kind() && a.raw_os_error() == b.raw_os_error()
            }
            (Self::Parse { detail: a }, Self::Parse { detail: b }) => a == b,
            _ => false,
        }
    }
}

impl Eq for TelemetryError {}

impl std::fmt::Display for TelemetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedHardware { vendor } => write!(f, "unsupported hardware: {vendor}"),
            Self::DriverMissing { driver } => write!(f, "required driver not loaded: {driver}"),
            Self::InsufficientPrivilege { hint } => write!(f, "insufficient privilege: {hint}"),
            Self::UnknownPmTableVersion { version } => {
                write!(f, "unknown SMU PM table version: 0x{version:08x}")
            }
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Parse { detail } => write!(f, "parse error: {detail}"),
        }
    }
}

impl std::error::Error for TelemetryError {
    /// Returns the inner `std::io::Error` for [`TelemetryError::Io`] and
    /// `None` for every other variant.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

/// The result type for every fallible telemetry operation.
///
/// Frozen (P2-02): each provider returns `TelemetryResult<T>`; unsupported /
/// privileged / unknown hardware states degrade to a structured `Err`,
/// never a panic.
pub type TelemetryResult<T> = std::result::Result<T, TelemetryError>;

/// A light reason tag displayed in a grid cell as `N/A (<reason>)`.
///
/// Frozen (P2-02). Deliberately distinct from [`TelemetryError`]: a cell
/// displays this compact tag, while [`TelemetryResult`] carries the rich
/// error with the full cause.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum NaReason {
    /// The detected hardware is not supported by this telemetry source.
    UnsupportedHardware,
    /// A required kernel driver is not loaded.
    DriverMissing,
    /// The operation requires more privilege than the caller has.
    InsufficientPrivilege,
    /// The SMU PM-table version is outside the supported layout set.
    UnknownPmTableVersion,
    /// This field does not apply to the detected platform (e.g. Intel
    /// voltages are out of Phase 2 scope).
    NotApplicable,
    /// A payload read from hardware was malformed.
    ParseError(String),
}

/// A single displayable telemetry cell: a value, or a structured "not
/// available" reason.
///
/// Frozen (P2-02). This is the no-panic contract for the whole crate: the
/// telemetry facade returns `Section<T>` per field so *any* unsupported /
/// privileged / unknown state degrades to [`Section::Na`] instead of
/// panicking.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Section<T> {
    /// The field was read successfully.
    Value(T),
    /// The field is not available; the cell displays the reason.
    Na(NaReason),
}

impl<T> Section<T> {
    /// The contained value, if this section is a [`Section::Value`].
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Value(v) => Some(v),
            Self::Na(_) => None,
        }
    }

    /// `true` if this section is a [`Section::Na`] (the inverse of
    /// [`Section::value`] returning `Some`).
    pub fn is_na(&self) -> bool {
        matches!(self, Self::Na(_))
    }

    /// Builds an N/A cell with the given [`NaReason`].
    pub fn na(reason: NaReason) -> Self {
        Self::Na(reason)
    }
}

impl<T> From<T> for Section<T> {
    /// Wraps a bare value as [`Section::Value`].
    fn from(value: T) -> Self {
        Self::Value(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    /// A representative `std::io::Error` for the `Io` variant in tests.
    fn io_err() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied")
    }

    /// Every frozen [`TelemetryError`] variant with representative data.
    fn all_variants() -> Vec<TelemetryError> {
        vec![
            TelemetryError::UnsupportedHardware {
                vendor: "Unknown".to_owned(),
            },
            TelemetryError::DriverMissing { driver: "ryzen_smu" },
            TelemetryError::InsufficientPrivilege { hint: "run as root" },
            TelemetryError::UnknownPmTableVersion { version: 0x0711_0300 },
            TelemetryError::Io(io_err()),
            TelemetryError::Parse {
                detail: "truncated PM blob at offset 0x1A".to_owned(),
            },
        ]
    }

    /// (a) `Display` text is non-empty and human-readable (contains
    /// alphabetic prose) for every variant.
    #[test]
    fn display_non_empty_for_every_variant() {
        for e in all_variants() {
            let text = e.to_string();
            assert!(!text.is_empty(), "Display must be non-empty: {e:?}");
            assert!(
                text.chars().any(char::is_alphabetic),
                "Display must be human-readable prose: {text:?} ({e:?})"
            );
        }
    }

    /// (a) Each variant's `Display` text is exactly the frozen,
    /// user-presentable form (what the P2-11 CLI renders).
    #[test]
    fn display_texts_are_frozen() {
        assert_eq!(
            TelemetryError::UnsupportedHardware {
                vendor: "arm".to_owned()
            }
            .to_string(),
            "unsupported hardware: arm"
        );
        assert_eq!(
            TelemetryError::DriverMissing { driver: "ryzen_smu" }.to_string(),
            "required driver not loaded: ryzen_smu"
        );
        assert_eq!(
            TelemetryError::InsufficientPrivilege { hint: "run as root" }.to_string(),
            "insufficient privilege: run as root"
        );
        assert_eq!(
            TelemetryError::UnknownPmTableVersion { version: 0x0711_0300 }.to_string(),
            "unknown SMU PM table version: 0x07110300"
        );
        assert_eq!(TelemetryError::Io(io_err()).to_string(), "I/O error: denied");
        assert_eq!(
            TelemetryError::Parse {
                detail: "bad header".to_owned()
            }
            .to_string(),
            "parse error: bad header"
        );
    }

    /// (b) `source()` is `Some` only for the `Io` variant, and that source
    /// is the inner `std::io::Error`.
    #[test]
    fn source_some_only_for_io() {
        let err = TelemetryError::Io(io_err());
        let src = err.source().expect("Io variant must expose a source");
        assert_eq!(
            src.to_string(),
            io_err().to_string(),
            "source must be the inner io::Error"
        );

        for e in [
            TelemetryError::UnsupportedHardware {
                vendor: "x".to_owned(),
            },
            TelemetryError::DriverMissing { driver: "ryzen_smu" },
            TelemetryError::InsufficientPrivilege { hint: "run as root" },
            TelemetryError::UnknownPmTableVersion { version: 1 },
            TelemetryError::Parse {
                detail: "d".to_owned(),
            },
        ] {
            assert!(e.source().is_none(), "expected no source for {e:?}");
        }
    }

    /// (c) `value()` is `Some` for `Value` and `None` for `Na`;
    /// `is_na()` is the inverse.
    #[test]
    fn section_value_is_na_inverse() {
        let v: Section<u32> = Section::Value(3200);
        assert_eq!(v.value(), Some(&3200));
        assert!(!v.is_na());

        let n: Section<u32> = Section::Na(NaReason::NotApplicable);
        assert_eq!(n.value(), None);
        assert!(n.is_na());
    }

    /// (d) `From<T>` wraps a bare value as `Section::Value`.
    #[test]
    fn from_t_wraps_as_value() {
        let s: Section<u16> = 16u16.into();
        assert_eq!(s, Section::Value(16));
        assert_eq!(s.value(), Some(&16));
        assert!(!s.is_na());

        let s: Section<String> = "SK hynix A-die".to_owned().into();
        assert_eq!(s, Section::Value("SK hynix A-die".to_owned()));
    }

    /// (e) `Section::na` builds the `Na` arm with the given reason.
    #[test]
    fn section_na_builds_na() {
        let n: Section<f32> = Section::na(NaReason::InsufficientPrivilege);
        assert!(n.is_na());
        assert_eq!(n.value(), None);
        assert_eq!(n, Section::Na(NaReason::InsufficientPrivilege));
    }

    /// (f) `NaReason` is `Clone` / `PartialEq` / `Debug` (compile-time
    /// derives, exercised at runtime).
    #[test]
    fn na_reason_clone_partial_eq_debug() {
        let a = NaReason::ParseError("truncated at 0x1A".to_owned());
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(b, NaReason::ParseError("truncated at 0x1A".to_owned()));
        assert_ne!(b, NaReason::ParseError("other".to_owned()));
        assert_eq!(NaReason::DriverMissing.clone(), NaReason::DriverMissing);
        assert_eq!(NaReason::NotApplicable, NaReason::NotApplicable);

        let dbg = format!("{b:?}");
        assert!(!dbg.is_empty());
        assert!(dbg.contains("ParseError"), "Debug must name the variant: {dbg}");

        for r in [
            NaReason::UnsupportedHardware,
            NaReason::DriverMissing,
            NaReason::InsufficientPrivilege,
            NaReason::UnknownPmTableVersion,
            NaReason::NotApplicable,
            NaReason::ParseError("x".to_owned()),
        ] {
            assert!(!format!("{r:?}").is_empty());
        }
    }

    /// `TelemetryError` is `Clone` + `PartialEq` + `Eq` (manual impls; the
    /// `Eq` bound is checked at compile time, the rest at runtime).
    #[test]
    fn telemetry_error_is_clone_partial_eq_eq() {
        fn bound<T: Clone + PartialEq + Eq>() {}
        bound::<TelemetryError>();

        let e = TelemetryError::DriverMissing { driver: "ryzen_smu" };
        assert_eq!(e.clone(), e);
        assert_ne!(e, TelemetryError::DriverMissing { driver: "i915" });
        assert_eq!(
            TelemetryError::UnknownPmTableVersion { version: 0x0711_0300 },
            TelemetryError::UnknownPmTableVersion { version: 0x0711_0300 }
        );
    }

    /// Cloning the `Io` variant preserves the source's kind + raw OS code
    /// (reconstruction, since `std::io::Error` is not `Clone`).
    #[test]
    fn telemetry_error_clone_io_preserves_source() {
        let raw = TelemetryError::Io(std::io::Error::from_raw_os_error(13));
        let cloned = raw.clone();
        assert_eq!(raw, cloned);
        assert_eq!(
            raw.source().expect("cloned raw err must expose a source").to_string(),
            cloned.source().expect("cloned raw err must expose a source").to_string()
        );

        let custom = TelemetryError::Io(io_err());
        let cloned = custom.clone();
        assert_eq!(custom, cloned);
        assert_eq!(
            custom.source().expect("cloned custom err must expose a source").to_string(),
            cloned.source().expect("cloned custom err must expose a source").to_string()
        );
    }

    /// The `TelemetryResult<T>` alias is the frozen result type for
    /// fallible operations.
    #[test]
    fn telemetry_result_alias() {
        fn ok() -> TelemetryResult<u32> {
            Ok(1)
        }
        fn err() -> TelemetryResult<u32> {
            Err(TelemetryError::DriverMissing { driver: "ryzen_smu" })
        }
        assert_eq!(ok(), Ok(1));
        assert!(err().is_err());
    }

    /// (P3-01) The no-panic contract types are wire-serializable:
    /// every `NaReason` arm — including the `ParseError(String)`
    /// payload arm — round-trips through bincode (the Phase 3 frame
    /// codec, plan D3).
    #[test]
    fn na_reason_bincode_round_trip() {
        let reasons = vec![
            NaReason::UnsupportedHardware,
            NaReason::DriverMissing,
            NaReason::InsufficientPrivilege,
            NaReason::UnknownPmTableVersion,
            NaReason::NotApplicable,
            NaReason::ParseError("truncated at 0x1A".to_owned()),
        ];
        let bytes = bincode::serialize(&reasons)
            .expect("NaReason must serialize (no-panic contract)");
        let back: Vec<NaReason> =
            bincode::deserialize(&bytes).expect("NaReason must deserialize");
        assert_eq!(reasons, back);
    }

    /// (P3-01) `Section<T>` round-trips through bincode for both
    /// arms (`Section::<u16>` fixture per the P3-01 scope boundary).
    #[test]
    fn section_bincode_round_trip() {
        let sections: Vec<Section<u16>> = vec![
            Section::Value(4800),
            Section::Na(NaReason::DriverMissing),
            Section::na(NaReason::ParseError("bad nibble".to_owned())),
        ];
        let bytes = bincode::serialize(&sections)
            .expect("Section must serialize (no-panic contract)");
        let back: Vec<Section<u16>> =
            bincode::deserialize(&bytes).expect("Section must deserialize");
        assert_eq!(sections, back);
    }
}
