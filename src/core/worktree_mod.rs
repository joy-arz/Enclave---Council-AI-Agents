use std::path::PathBuf;
use std::process::Command;
use tokio::fs;
use chrono::Local;

/// Manages git worktrees for isolated agent execution
#[derive(Debug, Clone)]
pub struct WorktreeManager {
    workspace_dir: PathBuf,
    worktrees_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Worktree {
    pub name: String,
    pub path: PathBuf,
    pub branch: String,
}

impl WorktreeManager {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self {
            workspace_dir: workspace_dir.clone(),
            worktrees_dir: workspace_dir.join(".enclave_worktrees"),
        }
    }

    /// Check if git repository exists in workspace
    pub fn is_git_repo(&self) -> bool {
        self.workspace_dir.join(".git").exists()
    }

    /// Create a new isolated worktree for a session
    pub async fn create_worktree(&self, session_id: &str) -> Result<Worktree, anyhow::Error> {
        if !self.is_git_repo() {
            return Err(anyhow::anyhow!("workspace is not a git repository"));
        }

        // Create worktrees directory
        fs::create_dir_all(&self.worktrees_dir).await?;

        let timestamp = Local::now().format("%Y%m%d_%H%M%S");
        let name = format!("session_{}_{}", &session_id[..session_id.len().min(8)], timestamp);
        let branch = format!("enclave/{}", name);
        let path = self.worktrees_dir.join(&name);

        // Check if worktree already exists
        if path.exists() {
            return Ok(Worktree {
                name,
                path,
                branch,
            });
        }

        // Create worktree using git
        let output = Command::new("git")
            .args(["worktree", "add", "-b", &branch, &path.to_string_lossy(), "HEAD"])
            .current_dir(&self.workspace_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("failed to create worktree: {}", stderr));
        }

        Ok(Worktree {
            name,
            path,
            branch,
        })
    }

    /// Remove a worktree when session ends
    pub async fn remove_worktree(&self, worktree: &Worktree) -> Result<(), anyhow::Error> {
        // Remove worktree using git
        let output = Command::new("git")
            .args(["worktree", "remove", "--force", &worktree.path.to_string_lossy()])
            .current_dir(&self.workspace_dir)
            .output()?;

        if !output.status.success() {
            // Try direct removal if git command fails
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("git worktree remove failed: {}, trying direct removal", stderr);
        }

        // Also remove the branch
        let _ = Command::new("git")
            .args(["branch", "-D", &worktree.branch])
            .current_dir(&self.workspace_dir)
            .output();

        // Remove the directory if it still exists
        if worktree.path.exists() {
            fs::remove_dir_all(&worktree.path).await?;
        }

        Ok(())
    }

    /// Get path for a specific worktree or fall back to main workspace
    pub fn get_execution_path(&self, worktree: Option<&Worktree>) -> PathBuf {
        worktree.map(|w| w.path.clone()).unwrap_or(self.workspace_dir.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktree_name_format() {
        let timestamp = Local::now().format("%Y%m%d_%H%M%S");
        let name = format!("session_abc123_{}", timestamp);
        assert!(name.starts_with("session_"));
    }
}
