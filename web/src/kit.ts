// The kit every app's web client builds on, served at /assets/kit.js.
//
// An app's bundle imports "ortus-kit" (kept external, mapped to
// /assets/kit.js), so every app shares the core's one copy of Lit, its
// translations, styles and API helpers, and looks like the rest of Ortus.
//
//   import { LitElement, html, css, customElement, LocalizeController, appText } from "ortus-kit";
//
// Everything exported here is a promise to the apps: change it with care.
export { LitElement, css, html, nothing, svg, unsafeCSS, type PropertyValues, type TemplateResult } from "lit";
export { customElement, property, query, state } from "lit/decorators.js";
export { unsafeSVG } from "lit/directives/unsafe-svg.js";
export { repeat } from "lit/directives/repeat.js";
export { LocalizeController, type Message } from "@lit-material/core/localize.js";
export { ApiError, call, me, session, setPlace, type Me } from "./api.js";
export { appText, registerAppMessages, LOCALES, type AppCatalogs } from "./i18n.js";
export { MARK, controls, page } from "./ui.js";
