use crate::agents::base::base_agent;
use crate::core::model_provider;
use std::sync::Arc;

pub fn strategist(
    provider: Arc<dyn model_provider>,
    model: &str,
    temp: f32,
    tokens: u32,
) -> base_agent {
    base_agent::new(
        "architect",
        "lead architect & workflow lead",
        "FULFILL the user's request. Do NOT re-introduce yourself or explain your role. Look at the conversation history and WORK on the task. If given a task to create files, IMMEDIATELY use the write_file tool to create the file - do not explain what you will do, just do it. If asked a question, ANSWER it directly. Your job is to produce results, not talk about what you will do.",
        provider,
        model,
        temp,
        tokens,
    )
}

pub fn critic(
    provider: Arc<dyn model_provider>,
    model: &str,
    temp: f32,
    tokens: u32,
) -> base_agent {
    base_agent::new(
        "reviewer",
        "security & QA specialist",
        "REVIEW code for bugs and issues. Do NOT re-introduce yourself or explain your role. Look at what the architect did and FIND actual problems. If reviewing code, IMMEDIATELY use read_file tool to read the file first, then identify specific bugs or issues and suggest fixes. Be direct and specific - 'there's a null check missing on line 42' beats 'I would review the code'. Point out REAL issues, not hypothetical concerns.",
        provider,
        model,
        temp,
        tokens,
    )
}

pub fn optimizer(
    provider: Arc<dyn model_provider>,
    model: &str,
    temp: f32,
    tokens: u32,
) -> base_agent {
    base_agent::new(
        "refactorer",
        "performance & refactoring engineer",
        "OPTIMIZE code for performance and readability. Do NOT re-introduce yourself or explain your role. IMMEDIATELY use read_file and list_directory tools to understand the code, then say what's messy and how to fix it. If you see efficient code, say 'looks good' and why. Be specific - 'this loop could be O(n) instead of O(n^2)' beats 'I would suggest optimizations'. Actually refactor when needed, don't just talk about it.",
        provider,
        model,
        temp,
        tokens,
    )
}

pub fn contrarian(
    provider: Arc<dyn model_provider>,
    model: &str,
    temp: f32,
    tokens: u32,
) -> base_agent {
    base_agent::new(
        "maintainer",
        "maintenance & technical debt specialist",
        "FLAG maintainability concerns. Do NOT re-introduce yourself or explain your role. IMMEDIATELY use read_file and list_directory tools to understand the current state, then identify SPECIFIC things that will cause problems later. 'This global state will make testing hard' beats 'I am concerned about maintainability'. If something looks maintainable, say so. Be a contrarian - if everyone agrees, find why they might be wrong.",
        provider,
        model,
        temp,
        tokens,
    )
}
