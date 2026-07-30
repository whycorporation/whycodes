//! Deterministic risk classification for shell commands.
//!
//! # Why this exists
//!
//! `ShellTool` runs whatever command string the model produces. The only gate
//! is the `[permission]` map, which decides per tool *name*, before the command
//! is known. Neither setting it offers is usable on its own: `allow` obeys a
//! model that emits a recursive delete of the home directory, and `ask` prompts
//! on every `ls` until the user turns it off — which converges on `allow`.
//!
//! This crate classifies the command itself, so `allow` can stay on for the
//! common case while destructive commands still stop.
//!
//! # Design
//!
//! **Classify by blast radius, not by command name.** A denylist of `rm -rf`
//! misses `find -delete`, `shred`, `dd` and `> file`. The question asked here
//! is what a command would destroy and whether it can be undone.
//!
//! **Escalate when parsing is ambiguous.** A false positive costs one
//! confirmation; a false negative costs a home directory. Command substitution,
//! unexpandable variables and unbalanced quotes all raise the level rather than
//! taking the benign reading.
//!
//! # Honest limitations
//!
//! This is defence in depth, not a sandbox.
//!
//! - An unrecognised command is [`RiskLevel::Safe`]. A shell script that
//!   deletes a home directory is invisible here, because the alternative —
//!   treating everything unknown as dangerous — prompts on every build and
//!   trains users to disable the feature.
//! - A determined `sh -c "$(printf …)"` defeats any static parser. That is why
//!   the catastrophic tier is a small, absolute, path-based check that does not
//!   depend on parsing the command correctly, and why it is refused outright
//!   rather than prompted.
//! - Classification never runs the command, resolves a symlink, or touches the
//!   network. It is a pure function of the string and the working directory.

mod assess;
mod paths;
mod tokenize;

pub use assess::{Assessment, RiskLevel, assess, assess_with_home};
pub use paths::PathScope;

/// The level at which whycode starts asking for confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RiskThreshold {
    /// Confirm anything that writes, including inside the project.
    Caution,
    /// Confirm only what reaches outside the project or cannot be undone.
    #[default]
    Destructive,
    /// Never confirm. [`RiskLevel::Catastrophic`] is still refused.
    Off,
}

impl std::str::FromStr for RiskThreshold {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "caution" => Ok(Self::Caution),
            "destructive" => Ok(Self::Destructive),
            "off" | "none" => Ok(Self::Off),
            other => Err(format!(
                "unknown risk threshold '{other}' (expected caution, destructive or off)"
            )),
        }
    }
}

/// What the caller should do with a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Run it without asking.
    Allow,
    /// Ask the user first, showing `reason`.
    Confirm { reason: String },
    /// Refuse. Not promptable — the user cannot approve their way past this.
    Refuse { reason: String },
}

/// Apply `threshold` to an assessment.
///
/// [`RiskLevel::Catastrophic`] always refuses, whatever the threshold. That is
/// the point of the tier: a setting a user chose for convenience must not be
/// able to authorise deleting their home directory.
pub fn decide(assessment: &Assessment, threshold: RiskThreshold) -> Decision {
    let reason = assessment
        .reason
        .clone()
        .unwrap_or_else(|| "destructive command".to_string());

    if assessment.level == RiskLevel::Catastrophic {
        return Decision::Refuse { reason };
    }

    let confirm_at = match threshold {
        RiskThreshold::Caution => RiskLevel::Caution,
        RiskThreshold::Destructive => RiskLevel::Destructive,
        RiskThreshold::Off => return Decision::Allow,
    };

    if assessment.level >= confirm_at {
        Decision::Confirm { reason }
    } else {
        Decision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::str::FromStr;

    fn decide_for(cmd: &str, threshold: RiskThreshold) -> Decision {
        let a = assess_with_home(
            cmd,
            &PathBuf::from("/work/proj"),
            Some(&PathBuf::from("/home/user")),
        );
        decide(&a, threshold)
    }

    #[test]
    fn catastrophic_is_refused_at_every_threshold() {
        for threshold in [
            RiskThreshold::Caution,
            RiskThreshold::Destructive,
            RiskThreshold::Off,
        ] {
            assert!(
                matches!(decide_for("rm -rf ~", threshold), Decision::Refuse { .. }),
                "{threshold:?}"
            );
        }
    }

    #[test]
    fn default_threshold_allows_in_project_cleanup() {
        assert_eq!(
            decide_for("rm -rf target", RiskThreshold::default()),
            Decision::Allow
        );
    }

    #[test]
    fn default_threshold_confirms_outside_the_project() {
        assert!(matches!(
            decide_for("rm -rf /tmp/x", RiskThreshold::default()),
            Decision::Confirm { .. }
        ));
    }

    #[test]
    fn caution_threshold_confirms_in_project_cleanup() {
        assert!(matches!(
            decide_for("rm -rf target", RiskThreshold::Caution),
            Decision::Confirm { .. }
        ));
    }

    #[test]
    fn off_threshold_still_allows_safe_and_destructive() {
        assert_eq!(decide_for("ls", RiskThreshold::Off), Decision::Allow);
        assert_eq!(
            decide_for("rm -rf /tmp/x", RiskThreshold::Off),
            Decision::Allow
        );
    }

    #[test]
    fn safe_commands_are_allowed_everywhere() {
        for threshold in [
            RiskThreshold::Caution,
            RiskThreshold::Destructive,
            RiskThreshold::Off,
        ] {
            assert_eq!(decide_for("cargo build", threshold), Decision::Allow);
        }
    }

    #[test]
    fn confirm_and_refuse_carry_a_reason() {
        match decide_for("rm -rf ~", RiskThreshold::Destructive) {
            Decision::Refuse { reason } => assert!(reason.contains('~')),
            other => panic!("expected refusal, got {other:?}"),
        }
        match decide_for("rm -rf /tmp/x", RiskThreshold::Destructive) {
            Decision::Confirm { reason } => assert!(reason.contains("/tmp/x")),
            other => panic!("expected confirmation, got {other:?}"),
        }
    }

    #[test]
    fn threshold_parses_from_config_strings() {
        assert_eq!(
            RiskThreshold::from_str("caution").unwrap(),
            RiskThreshold::Caution
        );
        assert_eq!(
            RiskThreshold::from_str("DESTRUCTIVE").unwrap(),
            RiskThreshold::Destructive
        );
        assert_eq!(RiskThreshold::from_str("off").unwrap(), RiskThreshold::Off);
        assert!(RiskThreshold::from_str("nonsense").is_err());
    }

    #[test]
    fn threshold_default_is_destructive() {
        assert_eq!(RiskThreshold::default(), RiskThreshold::Destructive);
    }
}
