// Iroh Gossip Chat — Tauri frontend JavaScript
//
// Communicates with the Rust backend via Tauri's IPC (`invoke`) and
// receives real-time events via `listen`.

// Tauri API — imported as ESM from the Tauri runtime
const { invoke } = window.__TAURI__ ? window.__TAURI__.core : {};
const { listen } = window.__TAURI__ ? window.__TAURI__.event : {};

// DOM refs
const landing = document.getElementById('landing');
const chat = document.getElementById('chat');
const messageList = document.getElementById('message-list');
const messageInput = document.getElementById('message-input');
const sendBtn = document.getElementById('send-btn');
const createRoomBtn = document.getElementById('create-room-btn');
const joinRoomBtn = document.getElementById('join-room-btn');
const joinTicketInput = document.getElementById('join-ticket-input');
const copyTicketBtn = document.getElementById('copy-ticket-btn');
const settingsBtn = document.getElementById('settings-btn');
const settingsModal = document.getElementById('settings-modal');
const closeSettingsBtn = document.getElementById('close-settings-btn');
const nicknameInput = document.getElementById('nickname-input');
const setNameBtn = document.getElementById('set-name-btn');
const chatTitle = document.getElementById('chat-title');
const connectionStatus = document.getElementById('connection-status');
const peerCount = document.getElementById('peer-count');
const statusMsg = document.getElementById('status-msg');
const toastContainer = document.getElementById('toast-container');

// Frontend state
let state = {
  roomTicket: null,
  entries: [],
  nickname: null,
  initialized: false,
};

// ── Init ──

async function init() {
  if (!invoke || !listen) {
    showToast('error', 'Tauri runtime not found. Run this as a desktop app.');
    return;
  }

  try {
    const result = await invoke('init_backend');
    console.log('Backend:', result);
    state.initialized = true;
  } catch (e) {
    // Backend may already be initialized (from lib.rs setup)
    console.log('Init note:', e);
    state.initialized = true;
  }

  // Set up event listeners
  if (listen) {
    listen('chat-new-entry', (e) => onNewEntry(e.payload));
    listen('chat-status', (e) => onStatusUpdate(e.payload));
    listen('chat-ticket', (e) => onTicket(e.payload));
    listen('chat-topic', (e) => onTopic(e.payload));
    listen('chat-nickname', (e) => onNickname(e.payload));
    listen('chat-disconnected', () => onDisconnected());
    listen('chat-error', (e) => onError(e.payload));
  }
}

// ── Event Handlers ──

function onNewEntry(entry) {
  if (!entry) return;
  state.entries.push(entry);
  appendMessage(entry);
}

function onStatusUpdate(status) {
  if (!status) return;
  connectionStatus.textContent = status.connected ? 'Connected' : 'Disconnected';
  connectionStatus.className = `badge ${status.connected ? 'online' : 'offline'}`;
  peerCount.textContent = `${status.peer_count || 0} peers · ${status.direct_peers || 0} direct`;
}

function onTicket(payload) {
  if (!payload || !payload.ticket) return;
  state.roomTicket = payload.ticket;
  showToast('info', 'Room created! Copy the ticket to share.');
}

function onTopic(payload) {
  if (!payload || !payload.topic) return;
  chatTitle.textContent = `Room: ${payload.topic.slice(0, 16)}...`;
}

function onNickname(payload) {
  if (!payload || !payload.name) return;
  state.nickname = payload.name;
  nicknameInput.value = payload.name;
}

function onDisconnected() {
  showToast('error', 'Disconnected from gossip mesh');
}

function onError(payload) {
  if (!payload || !payload.message) return;
  showToast('error', payload.message);
}

// ── UI Helpers ──

function appendMessage(entry) {
  const el = document.createElement('div');
  el.className = `message ${entry.kind || 'system'}`;

  if (entry.kind === 'remote') {
    const sender = document.createElement('div');
    sender.className = 'sender';
    sender.textContent = entry.label || 'Unknown';
    el.appendChild(sender);
  }

  const body = document.createElement('div');
  body.className = 'body';
  body.textContent = entry.body || '';
  el.appendChild(body);

  messageList.appendChild(el);
  el.scrollIntoView({ behavior: 'smooth', block: 'end' });
}

function clearMessages() {
  messageList.innerHTML = '';
  state.entries = [];
}

function showToast(type, message) {
  const el = document.createElement('div');
  el.className = `toast ${type}`;
  el.textContent = message;
  toastContainer.appendChild(el);
  setTimeout(() => { el.remove(); }, 4000);
}

function switchToChat() {
  landing.classList.add('hidden');
  chat.classList.remove('hidden');
  messageInput.focus();
}

function switchToLanding() {
  chat.classList.add('hidden');
  landing.classList.remove('hidden');
}

// ── Actions ──

async function doCreateRoom() {
  try {
    statusMsg.textContent = 'Creating room...';
    const ticket = await invoke('create_room');
    state.roomTicket = ticket;
    statusMsg.textContent = `Room ready! Ticket: ${ticket.slice(0, 30)}...`;

    // Load initial entries
    const entries = await invoke('get_entries');
    clearMessages();
    entries.forEach(e => {
      state.entries.push(e);
      appendMessage(e);
    });

    // Get status
    const status = await invoke('get_status');
    if (status) {
      connectionStatus.textContent = status.connected ? 'Connected' : 'Disconnected';
      connectionStatus.className = `badge ${status.connected ? 'online' : 'offline'}`;
    }

    switchToChat();
    showToast('success', 'Room created! Share the ticket with others.');
  } catch (e) {
    statusMsg.textContent = `Error: ${e}`;
    showToast('error', `Failed to create room: ${e}`);
  }
}

async function doJoinRoom() {
  const ticket = joinTicketInput.value.trim();
  if (!ticket) {
    showToast('error', 'Please enter a ticket');
    return;
  }

  try {
    statusMsg.textContent = 'Joining room...';
    await invoke('join_room', { ticket });

    // Load entries
    const entries = await invoke('get_entries');
    clearMessages();
    entries.forEach(e => {
      state.entries.push(e);
      appendMessage(e);
    });

    // Get status
    const status = await invoke('get_status');
    if (status) {
      connectionStatus.textContent = status.connected ? 'Connected' : 'Disconnected';
      connectionStatus.className = `badge ${status.connected ? 'online' : 'offline'}`;
    }

    switchToChat();
    showToast('success', 'Joined room!');
  } catch (e) {
    statusMsg.textContent = `Error: ${e}`;
    showToast('error', `Failed to join room: ${e}`);
  }
}

async function doSendMessage() {
  const text = messageInput.value.trim();
  if (!text) return;

  messageInput.value = '';
  try {
    await invoke('send_message', { text });
  } catch (e) {
    showToast('error', `Send failed: ${e}`);
  }
}

async function doCopyTicket() {
  if (!state.roomTicket) {
    // Try fetching it
    try {
      state.roomTicket = await invoke('get_ticket');
    } catch (_) {
      showToast('error', 'No ticket available');
      return;
    }
  }

  try {
    await navigator.clipboard.writeText(state.roomTicket);
    showToast('success', 'Ticket copied to clipboard');
  } catch (_) {
    // Fallback
    const ta = document.createElement('textarea');
    ta.value = state.roomTicket;
    document.body.appendChild(ta);
    ta.select();
    document.execCommand('copy');
    ta.remove();
    showToast('success', 'Ticket copied to clipboard');
  }
}

async function doSetNickname() {
  const name = nicknameInput.value.trim();
  if (!name) {
    showToast('error', 'Please enter a name');
    return;
  }

  try {
    await invoke('set_nickname', { name });
    showToast('success', `Nickname set to "${name}"`);
  } catch (e) {
    showToast('error', `Failed to set nickname: ${e}`);
  }
}

// ── Event Listeners ──

createRoomBtn.addEventListener('click', doCreateRoom);
joinRoomBtn.addEventListener('click', doJoinRoom);

joinTicketInput.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') doJoinRoom();
});

sendBtn.addEventListener('click', doSendMessage);
messageInput.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') doSendMessage();
});

copyTicketBtn.addEventListener('click', doCopyTicket);

settingsBtn.addEventListener('click', () => {
  settingsModal.classList.remove('hidden');
});

closeSettingsBtn.addEventListener('click', () => {
  settingsModal.classList.add('hidden');
});

setNameBtn.addEventListener('click', doSetNickname);

nicknameInput.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') doSetNickname();
});

// Close modal on backdrop click
settingsModal.addEventListener('click', (e) => {
  if (e.target === settingsModal) settingsModal.classList.add('hidden');
});

// ── Boot ──

document.addEventListener('DOMContentLoaded', init);
