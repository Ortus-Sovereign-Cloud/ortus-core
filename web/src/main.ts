import { startLocale } from "./i18n.js";
import "./components/ortus-root.js";

// English shows at once; the person's language replaces it when its
// catalog has loaded.
void startLocale();
