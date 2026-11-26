import english from "../../src/bin/boru/locales/en.json";
import french from "../../src/bin/boru/locales/fr.json";

type Locale = Record<string, string>;
const dictionaries: Record<string, Locale> = { en: english as Locale, fr: french as Locale };

/** Translate using the same key dictionaries as the Iced frontend. */
export function t(key: string, args: Record<string, string | number> = {}): string {
  const language = (navigator.language || "en").split("-")[0].toLowerCase();
  let value = dictionaries[language]?.[key] ?? (english as Locale)[key] ?? key;
  for (const [name, replacement] of Object.entries(args)) {
    value = value.split(`{${name}}`).join(String(replacement));
  }
  return value;
}

export const reactMessages: Record<string, string> = {
  "app.tagline": "Peer-to-peer, by design", "app.navigation": "Main navigation", "app.connected": "Connected", "app.connecting": "Connecting…", "app.workspace": "WORKSPACE", "app.chats": "Chats", "app.sharedMedia": "Shared media", "app.settings": "Settings", "app.connectedTo": "Connected to Boru", "app.connectingTo": "Connecting to Boru", "app.connectingConversations": "Connecting to your conversations…",
  "settings.appearance": "Appearance", "settings.loading": "Loading saved preferences…", "settings.theme": "Theme", "settings.light": "Light", "settings.dark": "Dark", "settings.amoled": "AMOLED", "settings.fontSize": "Font size", "settings.scale": "Scale", "settings.accent": "Accent", "settings.notificationsPrivacy": "Notifications & privacy", "settings.notificationPolicy": "Notification policy", "settings.allMessages": "All messages", "settings.mentionsOnly": "Mentions only", "settings.muted": "Muted", "settings.notificationSounds": "Notification sounds", "settings.showPresence": "Show presence indicator", "settings.typing": "Send typing indicators", "settings.shareAddresses": "Share direct addresses in invitations", "settings.network": "Network", "settings.keepRunning": "Keep network host running after closing the window", "settings.restartRequired": "restart required", "settings.restartOptions": "Restart-required options: {options}. Command-line and environment overrides take precedence over saved preferences.", "settings.saveFailed": "Settings save failed: {error}", "settings.immediate": "Appearance changes apply immediately. Network and identity settings are managed by the existing Boru configuration; changes requiring restart are shown in their respective controls.",
  "media.title": "Media transfer", "media.description": "Upload media or open an opaque handle to preview and download it.", "media.choose": "Choose media", "media.handle": "Opaque media handle", "media.paste": "Paste media handle", "media.type": "Media type", "media.size": "Media size in bytes", "media.add": "Add handle", "media.empty": "No media handles yet. Upload a file or paste a handle.", "media.previewOf": "Preview of {name}", "media.previewMissing": "Image preview not loaded", "media.reload": "Reload preview", "media.preview": "Preview", "media.working": "Working…", "media.download": "Download", "media.progress": "Download progress for {name}", "media.cancel": "Cancel", "media.cancelled": "Transfer cancelled", "media.failed": "{code}: {message}",
  "room.rooms": "Rooms", "room.empty": "No rooms yet.", "room.conversation": "Room conversation", "room.participants": "{count} participants", "room.loading": "Loading messages…", "room.messagesEmpty": "No messages yet. Start the conversation.", "room.you": "You", "room.message": "Message", "room.write": "Write a message…", "room.sending": "Sending…", "room.send": "Send", "room.choose": "Choose a room to view its history.", "room.unread": "{count} unread",
  "dialogs.gallery": "Dialog gallery", "dialogs.heading": "Dialogs & overlays", "dialogs.confirmation": "Confirmation", "dialogs.security": "Security", "dialogs.error": "Error", "dialogs.file": "File", "dialogs.deleteTitle": "Delete conversation?", "dialogs.deleteDescription": "This permanently removes the local conversation history.", "dialogs.approveTitle": "Approve peer?", "dialogs.approveDescription": "Verify this peer's identity before granting access.", "dialogs.failedTitle": "Operation failed", "dialogs.failedDescription": "The file could not be opened. Check that it is available and try again.", "dialogs.replaceTitle": "Replace existing file?", "dialogs.replaceDescription": "A file with this name already exists in the selected destination.", "dialogs.cancel": "Cancel", "dialogs.confirm": "Confirm", "dialogs.approve": "Approve", "dialogs.deny": "Deny", "dialogs.close": "Close", "dialogs.keepBoth": "Keep both", "dialogs.replace": "Replace", "dialogs.delete": "Delete", "dialogs.approvePeer": "Approve peer",
  "errors.recovery": "Reload the view. If the problem persists, restart Boru.", "errors.title": "Something went wrong", "errors.description": "This view could not be displayed.", "errors.retry": "Try again", "errors.reload": "Reload Boru", "errors.dismissLabel": "Dismiss error", "errors.dismiss": "Dismiss",
  "benchmark.title": "WebKitGTK media benchmark (test surface)", "benchmark.label": "Benchmark", "benchmark.running": "Benchmark running…", "benchmark.run": "Run 5-trial benchmark", "benchmark.export": "Export raw JSON", "benchmark.description": "One warm-up followed by five trials; media bytes use the real opaque-handle IPC.",
};
export function rt(key: string, args: Record<string, string | number> = {}): string {
  const language = (typeof navigator === "undefined" ? "en" : navigator.language).split("-")[0].toLowerCase();
  let value = dictionaries[language]?.[`react.${key}`] ?? reactMessages[key] ?? t(`react.${key}`);
  for (const [name, replacement] of Object.entries(args)) value = value.split(`{${name}}`).join(String(replacement));
  return value;
}

export function localeDirection(_language = navigator.language): "ltr" {
  // Only English and French are currently supported; both are left-to-right.
  return "ltr";
}
