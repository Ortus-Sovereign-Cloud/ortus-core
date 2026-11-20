import type { Message } from "@lit-material/core/localize.js";

/** English: the source language. Every other catalog has exactly these keys. */
const en = {
  "shell.apps": "Apps",
  "shell.home": "Ortus home",
  "shell.allApps": "All apps",
  "shell.allAppsShort": "All apps",
  "shell.skip": "Skip to content",
  "language.label": "Language",
  "switcher.title": "All apps",
  "switcher.find": "Find an app",
  "switcher.count": { one: "{count} app", other: "{count} apps" },
  "switcher.empty": "No apps are installed yet.",
  "switcher.emptyHint": "An administrator can switch apps on in Admin.",
  "switcher.noMatch": "No app matches “{query}”.",
  "switcher.loadFailed": "Could not load your apps.",
  "switcher.retry": "Try again",
  "notInstalled.title": "This app is not installed",
  "notInstalled.body": "There is no app at {path} here, or an administrator has switched it off.",
  "notInstalled.back": "Show all apps",
  "group.communicate": "Communicate",
  "group.organise": "Organise",
  "group.create": "Create and record",
  "group.organisation": "Organisation",
  "app.mail": "Mail",
  "app.chat": "Chat",
  "app.calls": "Calls",
  "app.calendar": "Calendar",
  "app.tasks": "Tasks",
  "app.people": "People",
  "app.drive": "Drive",
  "app.docs": "Docs",
  "app.approvals": "Approvals",
  "app.admin": "Admin",
  "app.settings": "Settings",
} satisfies Record<string, Message>;

export type MessageKey = keyof typeof en;
export type Messages = Record<MessageKey, Message>;
export default en;
