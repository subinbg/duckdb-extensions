// The one call to the extension. Every request carries the access token.
// The requests and the shape of each answer are written down in src/protocol.rs.

const TOKEN_KEY = "webui-token";

/**
 * Take the token from the page's address, or from this tab's storage after a reload.
 * The token is removed from the address bar, so that a copied or bookmarked address does not carry it.
 */
function adoptToken() {
  const inAddress = new URLSearchParams(location.search).get("token");
  if (inAddress !== null) {
    sessionStorage.setItem(TOKEN_KEY, inAddress);
    history.replaceState(null, "", "/");
  }
  return sessionStorage.getItem(TOKEN_KEY);
}

export const token = adoptToken();

/** Send one request, such as `ask("page", { result, offset, limit })`, and return its answer. */
export async function ask(kind, fields) {
  const body = JSON.stringify(fields === undefined ? kind : { [kind]: fields });
  const response = await fetch("/api", { method: "POST", headers: { "X-Webui-Token": token }, body }).catch(() => {
    throw new Error("the server did not answer; is the DuckDB process still running?");
  });

  const answer = await response.json();
  if (!response.ok) throw new Error(answer.error);
  return answer;
}

/** Cancel the running cell and whatever is computed in the background. */
export const interrupt = () => ask("interrupt").catch(() => {});

/** Put text on the clipboard, which a browser offers only to a page from this machine or served over HTTPS. */
export const copy = (text) => navigator.clipboard?.writeText(text);
