// Helpers the browser test pages share.

export const log = (line) => (document.getElementById("log").textContent += line + "\n");

export function waitFor(node, type, predicate = () => true, ms = 20000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timed out waiting for ${type}`)), ms);
    node.addEventListener(type, function on({ detail }) {
      if (!predicate(detail)) return;
      clearTimeout(timer);
      node.removeEventListener(type, on);
      resolve(detail);
    });
  });
}

/// Invites until the member's post-session key package has arrived.
export async function invite(host, groupId, member) {
  for (let attempt = 0; ; attempt++) {
    try {
      return await host.inviteToGroup(groupId, member);
    } catch (error) {
      if (attempt > 50) throw error;
      await new Promise((r) => setTimeout(r, 200));
    }
  }
}

/// Runs `body(step)` and sets the title the runner reads: PASS, or
/// FAIL: <step>: <reason>.
export async function run(body) {
  const step = { name: "start" };
  try {
    await body(step);
    document.title = "PASS";
  } catch (error) {
    log(String(error?.stack ?? error));
    document.title = `FAIL: ${step.name}: ${error?.message ?? error}`;
  }
}
