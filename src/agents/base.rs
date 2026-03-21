use std::sync::Arc;
use crate::core::model_provider;

#[allow(non_camel_case_types)]
pub struct base_agent {
    pub name: String,
    pub role: String,
    pub system_prompt: String,
    pub provider: Arc<dyn model_provider>,
    pub model_name: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub is_autonomous: bool,
}

#[allow(non_camel_case_types)]
impl base_agent {
    pub fn new(
        name: &str,
        role: &str,
        system_prompt: &str,
        provider: Arc<dyn model_provider>,
        model_name: &str,
        temperature: f32,
        max_tokens: u32,
    ) -> Self {
        Self {
            name: name.to_string(),
            role: role.to_string(),
            system_prompt: system_prompt.to_string(),
            provider,
            model_name: model_name.to_string(),
            temperature,
            max_tokens,
            is_autonomous: false,
        }
    }

    pub fn set_autonomous(&mut self, value: bool) {
        self.is_autonomous = value;
    }

    pub fn build_full_system_prompt(&self) -> String {
        let safety_instruction = if self.is_autonomous {
            "you are in AUTONOMOUS mode. CRITICAL: when using CLI tools that require approval (like qwen-cli), you MUST append the `-y` flag to bypass interactive mode. example: `qwen -p 'do something' -y`. you are an active engineering partner with full permission to use your internal tools (write_file, replace, run_shell_command) to directly modify the codebase. your goal is to drive the workflow forward until the task is complete. you have full access to the workspace root and all its subfolders recursively. do not just talk; ACT by reading and editing any necessary files in the workspace (including nested folders). assume ALL changes are pre-accepted and proceed immediately with implementation."
        } else {
            "you are in PROPOSAL mode. you must not modify files directly. instead, you should provide your suggested changes in your response using the following format:\n\n[PROPOSE_CHANGE:path/to/file]\n[new content of the file]\n[/PROPOSE_CHANGE]\n\nthe user will review these proposals and choose whether to apply them. you can propose multiple file changes in a single response."
        };

        format!(
            "you are a {}.\n\n{}\n\nresponsibilities:\n{}\n\nrules:\n* speak naturally as a human engineer in a casual conversation.\n* NEVER use bullet points, numbered lists, bold text, or rigid structural formats like 'Flaw/Fix' or '1. 2. 3.'.\n* write in plain paragraphs.\n* directly address the previous speaker like a real person.\n* the user's initial query is the absolute highest priority.\n* do not repeat previous points. add new insights.\n* be concise, direct, and conversational.\n* if the task requires code changes and you are in autonomous mode, use your tools to perform them immediately (recursively across all folders if needed). if in proposal mode, use the [PROPOSE_CHANGE] tag.\n",
            self.role, safety_instruction, self.system_prompt
        )
    }

    pub async fn get_response(&self, history: &str) -> Result<(String, String), anyhow::Error> {
        let prompt = format!(
            "current conversation history:\n{}\n\nplease provide your insights as the {}.",
            history, self.name
        );

        self.provider.call_model(
            &self.model_name,
            &prompt,
            Some(&self.build_full_system_prompt()),
            self.temperature,
            self.max_tokens,
        ).await
    }

    pub fn clone_for_parallel(&self) -> Self {
        Self {
            name: self.name.clone(),
            role: self.role.clone(),
            system_prompt: self.system_prompt.clone(),
            provider: self.provider.clone(),
            model_name: self.model_name.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            is_autonomous: self.is_autonomous,
        }
    }
}
