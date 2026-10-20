// Root entrypoint for a local OpenCode checkout install. OpenCode loads a
// directory plugin by falling back to `<dir>/index` (it does not read the
// package.json exports/main for a path spec), so this must resolve to the same
// default export as dist/plugin.js. The `setup` function belongs to W5.2.

import { default as plugin } from "./dist/plugin.js";

export default plugin;
export { plugin };
