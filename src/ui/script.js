const btn = document.getElementById('start-btn');
const browse_btn = document.getElementById('browse-btn');
const query_input = document.getElementById('query');
const workspace_input = document.getElementById('workspace');
const rounds_input = document.getElementById('rounds');
const autonomous_toggle = document.getElementById('autonomous-toggle');
const auto_rounds_toggle = document.getElementById('auto-rounds-toggle');
const feed = document.getElementById('feed');
const status_text = document.getElementById('status-text');
const status_dot = document.getElementById('status-dot');

// Toast Notification System
function showToast(message, type = 'info', duration = 4000) {
    const container = document.getElementById('toast-container');
    const toast = document.createElement('div');
    toast.className = `toast ${type}`;

    const iconMap = {
        success: '✓',
        error: '✕',
        warning: '⚠',
        info: 'ℹ'
    };

    toast.innerHTML = `
        <div class="toast-icon">${iconMap[type] || iconMap.info}</div>
        <span class="toast-message">${message}</span>
        <button class="toast-close" onclick="this.parentElement.remove()">×</button>
    `;

    container.appendChild(toast);

    if (duration > 0) {
        setTimeout(() => {
            toast.classList.add('removing');
            setTimeout(() => toast.remove(), 250);
        }, duration);
    }

    return toast;
}

// Agent binary inputs
const bin_inputs = {
    strategist: document.getElementById('bin-strategist'),
    critic: document.getElementById('bin-critic'),
    optimizer: document.getElementById('bin-optimizer'),
    maintainer: document.getElementById('bin-maintainer'),
    judge: document.getElementById('bin-judge')
};

const loading_indicator = document.getElementById('loading-indicator');
const loading_text = document.getElementById('loading-text');

let current_session_id = localStorage.getItem('enclave_session_id');
let last_workspace = localStorage.getItem('enclave_workspace');
let last_message_time = null;
let silence_check_interval = null;
let is_session_active = false;

if (last_workspace) {
    workspace_input.value = last_workspace;
}

// Restore agent binaries from localStorage
Object.keys(bin_inputs).forEach(role => {
    const saved = localStorage.getItem(`enclave_bin_${role}`);
    if (saved) bin_inputs[role].value = saved;
});

// Keyboard shortcuts
document.addEventListener('keydown', (e) => {
    // Cmd/Ctrl + Enter to send message
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        e.preventDefault();
        if (!btn.disabled && query_input.value.trim()) {
            btn.click();
        }
    }

    // Escape to clear input
    if (e.key === 'Escape' && document.activeElement === query_input) {
        query_input.value = '';
    }
});

// Test CLI functionality
document.querySelectorAll('.test-btn').forEach(btn => {
    btn.addEventListener('click', async () => {
        const role = btn.dataset.role;
        const command = bin_inputs[role].value.trim();
        if (!command) {
            showToast("Please enter a command first.", 'warning');
            return;
        }

        const workspace = workspace_input.value.trim();
        btn.textContent = "Testing...";
        btn.classList.remove('success', 'error');

        try {
            const response = await fetch('/api/test_cli', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ command, workspace_dir: workspace || null })
            });
            const res = await response.json();
            if (res.status === 'success') {
                btn.textContent = "Working";
                btn.classList.add('success');
            } else {
                btn.textContent = "Failed";
                btn.classList.add('error');
                console.error(`Test failed for ${role}:`, res.message);
            }
        } catch (err) {
            btn.textContent = "Failed";
            btn.classList.add('error');
            console.error(`Fetch error for ${role}:`, err);
        }

        // Reset button after 3 seconds
        setTimeout(() => {
            btn.textContent = "Test";
            btn.classList.remove('success', 'error');
        }, 3000);
    });
});

// on load, try to restore history if session exists
if (current_session_id) {
    restore_session(current_session_id);
}

// New Session button handler
document.getElementById('new-session-btn').addEventListener('click', () => {
    current_session_id = null;
    localStorage.removeItem('enclave_session_id');
    feed.innerHTML = `
        <div class="empty-state">
            <div class="empty-state-icon"><svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"></polygon></svg></div>
            <div class="empty-state-title">Ready to deliberate</div>
            <div class="empty-state-text">Describe a task or workflow below to begin.<br>Press <kbd style="background: var(--surface-elevated); padding: 2px 6px; border-radius: 3px; font-size: 0.75rem;">⌘</kbd> + <kbd style="background: var(--surface-elevated); padding: 2px 6px; border-radius: 3px; font-size: 0.75rem;">Enter</kbd> to send.</div>
        </div>
    `;
    update_status('System Idle', false);
    showToast('New session started', 'info');
});

// Clear Session button handler
document.getElementById('clear-session-btn').addEventListener('click', async () => {
    if (!current_session_id) {
        showToast('No active session to clear', 'warning');
        return;
    }

    // Clear the feed but keep the session ID for continuing
    feed.innerHTML = `
        <div class="empty-state">
            <div class="empty-state-icon"><svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"></polygon></svg></div>
            <div class="empty-state-title">Session cleared</div>
            <div class="empty-state-text">The session history has been cleared. Start a new task or continue the session below.</div>
        </div>
    `;

    // Clear session state but keep session ID for continuation
    update_status('Session Cleared', false);
    showToast('Session cleared. You can continue by asking another question.', 'success');
});

// Saved Sessions button handler
document.getElementById('saved-sessions-btn').addEventListener('click', async () => {
    openSessionsModal();
});

function openSessionsModal() {
    const modal = document.getElementById('saved-sessions-modal');
    modal.classList.remove('hidden');
    loadSessionsList();
}

function closeSessionsModal() {
    const modal = document.getElementById('saved-sessions-modal');
    modal.classList.add('hidden');
}

// Close modal when clicking backdrop
document.querySelector('.modal-backdrop')?.addEventListener('click', closeSessionsModal);

async function loadSessionsList() {
    const sessionsList = document.getElementById('sessions-list');
    sessionsList.innerHTML = '<div class="empty-state"><div class="empty-sessions-icon">⏳</div><div>Loading sessions...</div></div>';

    try {
        const response = await fetch('/api/sessions');
        const sessions = await response.json();

        if (sessions.length === 0) {
            sessionsList.innerHTML = `
                <div class="empty-sessions">
                    <div class="empty-sessions-icon"><svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 4h16c1.1 0 2 .9 2 2v12c0 1.1-.9 2-2 2H4c-1.1 0-2-.9-2-2V6c0-1.1.9-2 2-2z"></path><polyline points="22,6 12,13 2,6"></polyline></svg></div>
                    <div>No saved sessions yet</div>
                    <div style="font-size: 0.8rem; margin-top: 8px;">Start a new session to begin.</div>
                </div>
            `;
            return;
        }

        sessionsList.innerHTML = sessions.map(session => `
            <div class="session-item" onclick="continueSession('${session.session_id}')">
                <div class="session-item-header">
                    <span class="session-item-id">${session.session_id.substring(0, 8)}...</span>
                    <span class="session-item-messages">${session.message_count} messages</span>
                </div>
                <div class="session-item-preview">${escapeHtml(session.first_message)}</div>
                <div class="session-item-actions">
                    <button class="quick-action-btn" onclick="event.stopPropagation(); continueSession('${session.session_id}')">Continue</button>
                    <button class="quick-action-btn danger" onclick="event.stopPropagation(); deleteSession('${session.session_id}')">Delete</button>
                </div>
            </div>
        `).join('');
    } catch (err) {
        console.error("Failed to load sessions:", err);
        sessionsList.innerHTML = `
            <div class="empty-sessions">
                <div class="empty-sessions-icon">❌</div>
                <div>Failed to load sessions</div>
            </div>
        `;
    }
}

function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

async function continueSession(sessionId) {
    current_session_id = sessionId;
    localStorage.setItem('enclave_session_id', sessionId);
    closeSessionsModal();

    // Restore session from history
    update_status('Restoring session...', true);
    try {
        const response = await fetch(`/api/history/${sessionId}`);
        const history = await response.json();

        if (history.length > 0) {
            feed.innerHTML = '';
            history.forEach(msg => {
                append_message(msg.agent, msg.content, msg.round, msg.terminal_output);
            });
            update_status('System Idle', false);
            showToast('Session restored', 'success');
        } else {
            feed.innerHTML = `
                <div class="empty-state">
                    <div class="empty-state-icon"><svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"></polygon></svg></div>
                    <div class="empty-state-title">Session restored</div>
                    <div class="empty-state-text">The session has been restored. Ask a follow-up question to continue.</div>
                </div>
            `;
            update_status('System Idle', false);
        }
    } catch (err) {
        console.error("Failed to restore session:", err);
        update_status('System Idle', false);
        showToast('Failed to restore session', 'error');
    }
}

function check_silence() {
    if (!is_session_active || !last_message_time) return;

    const elapsed = Date.now() - last_message_time;
    const minutes = Math.floor(elapsed / 60000);

    if (elapsed > 60000) { // After 1 minute of silence
        if (minutes >= 60) {
            update_status(`Silent for ${Math.floor(minutes / 60)}h ${minutes % 60}m...`, true);
        } else {
            update_status(`Silent for ${minutes}m...`, true);
        }
    }

    if (elapsed > 300000) { // After 5 minutes, show warning
        showToast('Enclave seems stuck. May have failed silently.', 'warning', 8000);
    }
}

function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

function format_time(date) {
    const now = new Date();
    const diff = now - date;
    const seconds = Math.floor(diff / 1000);
    const minutes = Math.floor(seconds / 60);
    const hours = Math.floor(minutes / 60);

    if (seconds < 60) return 'just now';
    if (minutes < 60) return `${minutes}m ago`;
    if (hours < 24) return `${hours}h ago`;
    return date.toLocaleDateString();
}

async function deleteSession(sessionId) {
    if (!confirm('Are you sure you want to delete this session? This cannot be undone.')) {
        return;
    }

    try {
        const response = await fetch(`/api/sessions/${sessionId}`, {
            method: 'DELETE'
        });
        const result = await response.json();

        if (result.status === 'success') {
            showToast('Session deleted', 'success');
            loadSessionsList();

            // If we deleted the current session, clear it
            if (current_session_id === sessionId) {
                current_session_id = null;
                localStorage.removeItem('enclave_session_id');
                feed.innerHTML = `
                    <div class="empty-state">
                        <div class="empty-state-icon"><svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"></polygon></svg></div>
                        <div class="empty-state-title">Ready to deliberate</div>
                        <div class="empty-state-text">Describe a task or workflow below to begin.<br>Press <kbd style="background: var(--surface-elevated); padding: 2px 6px; border-radius: 3px; font-size: 0.75rem;">⌘</kbd> + <kbd style="background: var(--surface-elevated); padding: 2px 6px; border-radius: 3px; font-size: 0.75rem;">Enter</kbd> to send.</div>
                    </div>
                `;
            }
        } else {
            showToast('Failed to delete session', 'error');
        }
    } catch (err) {
        console.error("Failed to delete session:", err);
        showToast('Failed to delete session', 'error');
    }
}

browse_btn.addEventListener('click', async () => {
    try {
        const response = await fetch('/api/browse');
        const path = await response.json();
        if (path) {
            workspace_input.value = path;
            localStorage.setItem('enclave_workspace', path);
        }
    } catch (err) {
        console.error("failed to browse:", err);
    }
});

btn.addEventListener('click', () => {
    const query = query_input.value.trim();
    if (!query) return;

    const workspace = workspace_input.value.trim();
    const autonomous = autonomous_toggle.checked;
    const autoRounds = auto_rounds_toggle.checked;
    const rounds = rounds_input.value;

    if (workspace) {
        localStorage.setItem('enclave_workspace', workspace);
    }

    // Save binaries to localStorage
    Object.keys(bin_inputs).forEach(role => {
        localStorage.setItem(`enclave_bin_${role}`, bin_inputs[role].value);
    });

    btn.disabled = true;
    feed.innerHTML = '';

    // add user message to feed
    append_message('User', query, 0);
    query_input.value = '';

    update_status('Processing...', true);
    show_loading(true, 'Processing your request...');

    start_debate(query, current_session_id, autonomous, autoRounds, workspace, rounds);
});

query_input.addEventListener('keypress', (e) => {
    if (e.key === 'Enter') btn.click();
});

function show_loading(show, text = 'Waiting for your request...') {
    if (show) {
        loading_indicator.classList.remove('hidden');
        loading_indicator.style.display = 'flex';
        loading_text.textContent = text;
    } else {
        loading_indicator.classList.add('hidden');
        loading_indicator.style.display = 'none';
    }
}

function update_status(text, active = false) {
    status_text.textContent = text;
    if (active) {
        status_dot.classList.add('active');
    } else {
        status_dot.classList.remove('active');
    }
}

function start_debate(query, session_id, autonomous, autoRounds, workspace, rounds) {
    let url = "/api/enclave?query=" + encodeURIComponent(query);
    if (session_id) url += "&session_id=" + session_id;
    if (autonomous) url += "&autonomous=true";
    if (!autoRounds) url += "&auto_rounds=false";
    if (workspace) url += "&workspace_dir=" + encodeURIComponent(workspace);
    if (rounds) url += "&rounds=" + rounds;

    // Add binary overrides
    Object.keys(bin_inputs).forEach(role => {
        const val = bin_inputs[role].value.trim();
        if (val) {
            url += `&${role}_binary=${encodeURIComponent(val)}`;
        }
    });

    console.log("Starting session with URL:", url);
    const event_source = new EventSource(url);

    event_source.onopen = () => {
        console.log("SSE Connection opened.");
    };

    event_source.addEventListener('session_info', (event) => {
        const data = JSON.parse(event.data);
        console.log("Session info received:", data);
        current_session_id = data.session_id;
        localStorage.setItem('enclave_session_id', current_session_id);
    });

    let lead_engineer_received = false;
    is_session_active = true;
    last_message_time = Date.now();

    // Start silence detection
    if (silence_check_interval) clearInterval(silence_check_interval);
    silence_check_interval = setInterval(() => {
        check_silence();
    }, 15000); // Check every 15 seconds

    event_source.onmessage = (event) => {
        const data = JSON.parse(event.data);
        last_message_time = Date.now();
        show_loading(false);
        append_message(data.agent, data.content, data.round, data.terminal_output);
        update_status(data.agent + " is speaking...", true);
        show_loading(true, `Waiting for response...`);

        if (data.agent.toLowerCase().replace(/ /g, '.') === 'lead.engineer') {
            lead_engineer_received = true;
        }
    };

    event_source.onerror = (e) => {
        console.log("SSE Connection closed.");
        event_source.close();
        btn.disabled = false;
        is_session_active = false;
        if (silence_check_interval) {
            clearInterval(silence_check_interval);
            silence_check_interval = null;
        }
        show_loading(true, 'Waiting for your request...');

        if (lead_engineer_received) {
            update_status('Enclave Adjourned', false);
            // Optionally add a small notification card
            const done_div = document.createElement('div');
            done_div.style = "text-align: center; color: var(--accent); font-size: 0.75rem; font-weight: 600; margin: 24px auto; padding: 12px 24px; background: var(--accent-muted); border: 1px solid var(--accent); border-radius: var(--radius); max-width: 300px; text-transform: uppercase; letter-spacing: 0.08em;";
            done_div.textContent = "— Deliberation Complete —";
            feed.appendChild(done_div);
        } else {
            update_status('System Idle (Session Error)', false);
        }
    };
}

async function restore_session(session_id) {
    update_status('Restoring session...', true);
    try {
        const response = await fetch(`/api/history/${session_id}`);
        const history = await response.json();
        
        if (history.length > 0) {
            feed.innerHTML = '';
            history.forEach(msg => {
                append_message(msg.agent, msg.content, msg.round, msg.terminal_output);
            });
            update_status('System Idle', false);
        } else {
            localStorage.removeItem('enclave_session_id');
            update_status('System Idle', false);
        }
    } catch (err) {
        console.error("failed to restore session:", err);
        update_status('System Idle', false);
    }
}

function append_message(agent, content, round, terminal_output = "") {
    const card = document.createElement('div');
    const agent_lower = agent.toLowerCase().replace(/ /g, '.');
    card.className = "card " + agent_lower;

    const header = document.createElement('div');
    header.className = 'card-header';
    
    const agent_info = document.createElement('div');
    agent_info.className = 'agent-info';
    
    const icon = document.createElement('div');
    icon.className = 'agent-icon';
    icon.textContent = agent.charAt(0).toUpperCase();
    
    const name_span = document.createElement('span');
    name_span.className = 'agent-name';
    name_span.textContent = agent;
    
    agent_info.appendChild(icon);
    agent_info.appendChild(name_span);
    
    const round_tag = document.createElement('span');
    round_tag.className = 'round-tag';
    round_tag.textContent = round === 0 ? "Initial" : "Round " + round;

    const time_tag = document.createElement('span');
    time_tag.className = 'time-tag';
    time_tag.textContent = format_time(new Date());

    header.appendChild(agent_info);
    header.appendChild(round_tag);
    header.appendChild(time_tag);

    const body = document.createElement('div');
    body.className = 'card-content';
    
    // check if content is json (from judge)
    try {
        if (agent_lower === 'lead.engineer') {
            const clean_content = content.replace(/```json/gi, '').replace(/```/g, '').trim();
            const json = JSON.parse(clean_content);
            const status_color = json.final_decision === 'FINISHED' ? 'var(--accent)' : (json.final_decision === 'CONTINUE' ? 'var(--primary)' : 'var(--warning)');
            body.innerHTML = `
                <div class="verdict-grid">
                    <div class="verdict-item full-width">
                        <span class="verdict-label">Summary</span>
                        <div>${json.summary || 'N/A'}</div>
                    </div>
                    <div class="verdict-item">
                        <span class="verdict-label">Status</span>
                        <div style="color: ${status_color}; font-weight: 700; font-family: 'Fira Code';">${json.final_decision || 'UNKNOWN'}</div>
                    </div>
                    <div class="verdict-item">
                        <span class="verdict-label">Best Answer</span>
                        <div style="font-size: 0.8rem; opacity: 0.8;">${json.best_answer || 'N/A'}</div>
                    </div>
                    <div class="verdict-item full-width">
                        <span class="verdict-label">Reasoning</span>
                        <div style="font-style: italic; font-size: 0.85rem; color: var(--text-muted);">${json.reasoning || 'N/A'}</div>
                    </div>
                    <div class="verdict-item full-width">
                        <span class="verdict-label">Key Disagreements</span>
                        <ul style="margin: 0; padding-left: 18px; color: var(--text-muted); font-size: 0.85rem;">
                            ${(json.key_disagreements || []).map(d => `<li>${d}</li>`).join('')}
                        </ul>
                    </div>
                </div>
            `;
        } else if (agent_lower === 'user') {
            body.innerHTML = escapeHtml(content).replace(/\n/g, '<br>');
        } else {
            // Parse proposals in non-autonomous mode
            const proposals = parse_proposals(content);
            if (proposals.length > 0) {
                let clean_content = content;
                proposals.forEach(p => {
                    clean_content = clean_content.replace(p.raw, "");
                });

                body.innerHTML = escapeHtml(clean_content.trim()).replace(/\n/g, '<br>');

                proposals.forEach(p => {
                    const prop_div = document.createElement('div');
                    prop_div.className = 'proposal-box';

                    const prop_header = document.createElement('div');
                    prop_header.className = 'proposal-header';
                    prop_header.innerHTML = `<span><strong>PROPOSAL:</strong> ${escapeHtml(p.path)}</span>`;

                    const apply_btn = document.createElement('button');
                    apply_btn.textContent = "Apply Change";
                    apply_btn.className = "quick-action-btn";
                    apply_btn.style = "background: var(--accent); color: white; border-color: var(--accent);";
                    apply_btn.onclick = () => apply_proposed_change(p.path, p.content, apply_btn);

                    prop_header.appendChild(apply_btn);

                    const prop_body = document.createElement('pre');
                    prop_body.className = 'proposal-body';
                    prop_body.textContent = p.content;

                    prop_div.appendChild(prop_header);
                    prop_div.appendChild(prop_body);
                    body.appendChild(prop_div);
                });
            } else {
                body.innerHTML = escapeHtml(content).replace(/\n/g, '<br>');
            }
        }
    } catch (e) {
        body.innerHTML = escapeHtml(content).replace(/\n/g, '<br>');
    }

    // Add terminal output if present (collapsible)
    if (terminal_output) {
        const term_details = document.createElement('details');
        term_details.style = "margin-top: 16px; font-size: 0.75rem; border-top: 1px solid var(--border); padding-top: 12px;";
        const term_summary = document.createElement('summary');
        term_summary.textContent = "Terminal Logs & Thoughts";
        term_summary.style = "cursor: pointer; color: var(--text-muted); font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em;";
        
        const term_pre = document.createElement('pre');
        term_pre.style = "margin-top: 8px; padding: 12px; background: var(--bg); border-radius: var(--radius-sm); max-height: 400px; overflow-y: auto; overflow-x: hidden; color: #94a3b8; font-family: 'Fira Code', monospace; white-space: pre-wrap; overflow-wrap: anywhere;";
        term_pre.textContent = terminal_output;
        
        term_details.appendChild(term_summary);
        term_details.appendChild(term_pre);
        body.appendChild(term_details);
    }

    card.appendChild(header);
    card.appendChild(body);
    feed.appendChild(card);

    // auto-scroll
    feed.scrollTo({ top: feed.scrollHeight, behavior: 'smooth' });
}

function parse_proposals(text) {
    const proposals = [];
    const regex = /\[PROPOSE_CHANGE:(.*?)\]([\s\S]*?)\[\/PROPOSE_CHANGE\]/g;
    let match;
    while ((match = regex.exec(text)) !== null) {
        proposals.push({
            path: match[1].trim(),
            content: match[2].trim(),
            raw: match[0]
        });
    }
    return proposals;
}

async function apply_proposed_change(path, content, btn) {
    btn.disabled = true;
    btn.textContent = "Applying...";
    try {
        const response = await fetch('/api/apply', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ path, content })
        });
        const res = await response.json();
        if (res.status === 'success') {
            btn.textContent = "Applied";
            btn.style.background = "var(--text-muted)";
            showToast("Change applied successfully!", 'success');
        } else {
            showToast("Error: " + res.message, 'error');
            btn.disabled = false;
            btn.textContent = "Apply Change";
        }
    } catch (err) {
        showToast("Failed to apply change.", 'error');
        btn.disabled = false;
        btn.textContent = "Apply Change";
    }
}
