//! The opaque case reference that the account offboarding command and the
//! trade responsibility reassignment both record.
//!
//! One type serves both contracts, so the two routes accept exactly the same
//! values. Each caller maps a refusal to its own error body.

/// The longest case reference. It matches the database check on
/// `individual_offboardings.evidence_case_ref`.
pub const CASE_REFERENCE_MAX_LENGTH: usize = 64;

/// The ticket systems a case reference may name.
pub const CASE_REFERENCE_PREFIXES: [&str; 4] = ["CASE", "REQ", "SEC", "TICKET"];

/// At most this many digit groups follow the prefix.
const CASE_REFERENCE_MAX_GROUPS: usize = 3;

/// At most this many digits form one group.
const CASE_REFERENCE_MAX_GROUP_DIGITS: usize = 12;

/// A value outside the case-reference grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCaseReference;

/// An opaque reference to a verified request, kept outside Verdaxis.
///
/// The grammar is `PREFIX-DIGITS`, with `PREFIX` one of
/// [`CASE_REFERENCE_PREFIXES`] and then one to three groups of 1 to 12 digits
/// separated by `-` or `/`, for example `CASE-2026/0042` or `SEC-2026-11-0001`.
/// At most [`CASE_REFERENCE_MAX_LENGTH`] characters. Only digits follow the
/// prefix, so the field cannot carry a name, an email address or a narrative.
#[derive(Clone, PartialEq, Eq)]
pub struct CaseReference(String);

impl CaseReference {
    pub fn parse(value: &str) -> Result<Self, InvalidCaseReference> {
        if value.len() > CASE_REFERENCE_MAX_LENGTH {
            return Err(InvalidCaseReference);
        }
        let (prefix, identifier) = value.split_once('-').ok_or(InvalidCaseReference)?;
        if !CASE_REFERENCE_PREFIXES.contains(&prefix) {
            return Err(InvalidCaseReference);
        }
        let groups: Vec<&str> = identifier.split(['-', '/']).collect();
        let valid_groups = groups.len() <= CASE_REFERENCE_MAX_GROUPS
            && groups.iter().all(|group| {
                (1..=CASE_REFERENCE_MAX_GROUP_DIGITS).contains(&group.len())
                    && group.bytes().all(|byte| byte.is_ascii_digit())
            });
        if !valid_groups {
            return Err(InvalidCaseReference);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The reference is never written to a log line.
impl std::fmt::Debug for CaseReference {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CaseReference(REDACTED)")
    }
}
