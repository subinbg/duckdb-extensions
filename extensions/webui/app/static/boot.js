// Loads the application, which is the WebAssembly module next to this file.
import init from "./webui_app.js";

init().catch((error) => {
  document.getElementById("loading").textContent = `The application could not start: ${error}`;
});
