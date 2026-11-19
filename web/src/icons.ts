import { unsafeCSS, type CSSResult } from "lit";

// Generated from the App switcher board of the Ortus design canvas.
// One glyph per app, drawn on a 48 by 48 grid, and the tint of its tile.

export interface AppIcon {
  /** Dark colour the tile fades from. */
  tint: string;
  /** SVG content for a `0 0 48 48` view box. */
  svg: string;
}

export const APP_ICONS: Record<string, AppIcon> = {
  mail: { tint: "#1B2350", svg: "<rect x=\"5\" y=\"11\" width=\"38\" height=\"27\" rx=\"6\" fill=\"#8EA2FF\"/><path d=\"M9 16 L24 27 L39 16\" fill=\"none\" stroke=\"#ECEEF3\" stroke-width=\"3.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>" },
  chat: { tint: "#1B2350", svg: "<circle cx=\"25\" cy=\"22\" r=\"16\" fill=\"#8EA2FF\"/><path d=\"M12 31 L7 42 L20 36 Z\" fill=\"#8EA2FF\"/><circle cx=\"18\" cy=\"22\" r=\"2.4\" fill=\"#ECEEF3\"/><circle cx=\"25\" cy=\"22\" r=\"2.4\" fill=\"#ECEEF3\"/><circle cx=\"32\" cy=\"22\" r=\"2.4\" fill=\"#ECEEF3\"/>" },
  calls: { tint: "#1B2350", svg: "<rect x=\"5\" y=\"13\" width=\"27\" height=\"22\" rx=\"6\" fill=\"#8EA2FF\"/><path d=\"M34 21 L43 15 V33 L34 27 Z\" fill=\"#ECEEF3\"/>" },
  calendar: { tint: "#0E2F2E", svg: "<rect x=\"6\" y=\"8\" width=\"36\" height=\"34\" rx=\"7\" fill=\"#4FD1C5\"/><path d=\"M6 15 A7 7 0 0 1 13 8 H35 A7 7 0 0 1 42 15 V18 H6 Z\" fill=\"#ECEEF3\"/><rect x=\"27\" y=\"27\" width=\"8\" height=\"8\" rx=\"2\" fill=\"#0B0D14\"/>" },
  tasks: { tint: "#0E2F2E", svg: "<path d=\"M6 24 A18 18 0 0 1 24 6 H42 V24 A18 18 0 0 1 24 42 H6 Z\" fill=\"#4FD1C5\"/><path d=\"M15 24 L21.5 30.5 L33 19\" fill=\"none\" stroke=\"#ECEEF3\" stroke-width=\"4.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>" },
  people: { tint: "#0E2F2E", svg: "<circle cx=\"31\" cy=\"17\" r=\"6\" fill=\"#4FD1C5\" opacity=\".55\"/><path d=\"M24 41 A11 11 0 0 1 44 41 Z\" fill=\"#4FD1C5\" opacity=\".55\"/><circle cx=\"18\" cy=\"16\" r=\"8\" fill=\"#ECEEF3\"/><path d=\"M4 42 A14 14 0 0 1 32 42 Z\" fill=\"#4FD1C5\"/>" },
  drive: { tint: "#3A2412", svg: "<path d=\"M24 7 L43 17 L24 27 L5 17 Z\" fill=\"#ECEEF3\"/><path d=\"M5 25 L24 35 L43 25\" fill=\"none\" stroke=\"#F5A35C\" stroke-width=\"4.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/><path d=\"M5 32 L24 42 L43 32\" fill=\"none\" stroke=\"#F5A35C\" stroke-width=\"4.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\" opacity=\".55\"/>" },
  docs: { tint: "#3A2412", svg: "<path d=\"M10 9 A4 4 0 0 1 14 5 H28 L38 15 V39 A4 4 0 0 1 34 43 H14 A4 4 0 0 1 10 39 Z\" fill=\"#F5A35C\"/><path d=\"M28 5 V15 H38 Z\" fill=\"#ECEEF3\" opacity=\".7\"/><rect x=\"16\" y=\"22\" width=\"16\" height=\"3.5\" rx=\"1.75\" fill=\"#ECEEF3\"/><rect x=\"16\" y=\"29\" width=\"16\" height=\"3.5\" rx=\"1.75\" fill=\"#ECEEF3\"/><rect x=\"16\" y=\"36\" width=\"9\" height=\"3.5\" rx=\"1.75\" fill=\"#ECEEF3\"/>" },
  approvals: { tint: "#3A2412", svg: "<path d=\"M7 30 C12 14 17 38 23 24 S33 16 41 26\" fill=\"none\" stroke=\"#ECEEF3\" stroke-width=\"4\" stroke-linecap=\"round\"/><rect x=\"7\" y=\"37\" width=\"34\" height=\"4.5\" rx=\"2.25\" fill=\"#F5A35C\"/><path d=\"M33 6 L42 15 L36 21 L27 12 Z\" fill=\"#F5A35C\"/>" },
  admin: { tint: "#3A1616", svg: "<path d=\"M24 4 L41 10 V23 C41 33 34 40 24 44 C14 40 7 33 7 23 V10 Z\" fill=\"#FF7A6E\"/><g transform=\"translate(15 15) scale(.375)\"><path d=\"M22 30 L4 30 A18 18 0 0 1 22 12 Z\" fill=\"#ECEEF3\"/><path d=\"M26 30 L26 12 A18 18 0 0 1 44 30 Z\" fill=\"#ECEEF3\"/><rect x=\"4\" y=\"34\" width=\"40\" height=\"5\" rx=\"2.5\" fill=\"#ECEEF3\"/></g>" },
  settings: { tint: "#22252F", svg: "<circle cx=\"24\" cy=\"24\" r=\"17\" fill=\"#9198A8\"/><circle cx=\"24\" cy=\"24\" r=\"7\" fill=\"#ECEEF3\"/><path d=\"M24 7 V13 M24 35 V41 M7 24 H13 M35 24 H41\" stroke=\"#0B0D14\" stroke-width=\"4\"/>" },
};

/**
 * One rule per app that tints its tile. Written as a stylesheet, not as
 * style attributes: the page's content security policy allows no inline
 * style. The tints are constants from this file, never user input.
 */
export const tileTints: CSSResult = unsafeCSS(
  Object.entries(APP_ICONS)
    .map(([id, icon]) => `.tile[data-app="${id}"]{background:linear-gradient(155deg,${icon.tint},#0b0d14)}`)
    .join("\n"),
);
