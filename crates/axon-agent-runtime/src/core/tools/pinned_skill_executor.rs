//! Execution boundary for an already-pinned official Skill body.

use axon_crypto::sha256_hex;
use axon_domain::{AxonError, AxonResult, ToolOutput};

/// Immutable Skill material supplied by Axon runtime for one Attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedSkillBody {
    /// Official Skill manifest name.
    pub name: String,
    /// SHA-256 digest captured by the durable catalog snapshot.
    pub content_digest: String,
    /// Immutable Skill body copied into the Attempt input.
    pub content: String,
}

impl PinnedSkillBody {
    /// Validates that the body still matches the durable catalog digest.
    pub fn validate(&self) -> AxonResult<()> {
        if self.name.trim().is_empty() || self.content.is_empty() {
            return Err(AxonError::tool_invalid_definition(
                "pinned Skill name/content 不能为空",
            ));
        }
        if sha256_hex(self.content.as_bytes()) != self.content_digest {
            return Err(AxonError::tool_invalid_definition(
                "pinned Skill content digest 不匹配",
            ));
        }
        Ok(())
    }
}

/// Runtime that consumes only the immutable body passed by the Attempt snapshot.
#[derive(Debug, Default, Clone, Copy)]
pub struct PinnedSkillExecutor;

impl PinnedSkillExecutor {
    /// Validates and returns the pinned body without consulting live roots.
    pub fn invoke(&self, skill: &PinnedSkillBody) -> AxonResult<ToolOutput> {
        skill.validate()?;
        Ok(ToolOutput {
            text: skill.content.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_executor_rejects_tampered_body_without_live_reload() {
        let content = "# lint\nRun the checks.".to_owned();
        let mut skill = PinnedSkillBody {
            name: "lint".to_owned(),
            content_digest: sha256_hex(content.as_bytes()),
            content,
        };
        assert_eq!(
            PinnedSkillExecutor.invoke(&skill).unwrap().text,
            "# lint\nRun the checks."
        );
        skill.content.push_str("\nchanged");
        assert!(PinnedSkillExecutor.invoke(&skill).is_err());
    }
}
