// The durable store's files and key, opened in the worker before the engine.
//
// Invariants (docs/bridges/web.md, W8):
// - One node per account per origin. A Web Lock named for the account is
//   held for the worker's life, and a second node is refused before it opens
//   a file. The request waits up to LOCK_WAIT_MS rather than failing at
//   once: a worker the page terminated releases its lock and its file
//   handles a moment later, and a restart must not lose that race. Two engines over one log would each advance ratchet state the
//   other never sees, and the groups would diverge without an error. (An OPFS
//   sync handle also refuses a second opener, but only for the file it
//   guards, and only once the second node has started opening.)
// - Every file is opened before the engine starts. Opening is asynchronous
//   and the storage traits are synchronous, so the engine cannot open one
//   later.
// - The store key is never stored in the clear. It is sealed under a
//   non-extractable AES-GCM key kept in IndexedDB, and the sealed copy lives
//   beside the logs. Copying the logs and the sealed key does not open them.
//   A script on this origin can still ask the browser to unseal it: this is
//   protection for the files at rest, not a keystore.
// - A missing key never mints a new one over existing logs. If the sealed
//   key or its wrapping key is gone, or does not unseal, while a log is not
//   empty, the node refuses to start and changes nothing: a new key would
//   make the old logs unreadable, and they hold the identity. While every
//   log is empty there is nothing to lose (a crash while the key was first
//   written leaves exactly that), and a new key is minted.

const ROOT = "offline-protocol";
const LOG_FILES = ["mls.0", "mls.1", "state.0", "state.1"];
const SEALED_KEY_FILE = "store-key";
const KEY_DB = "offline-protocol-keys";
const KEY_STORE = "wrapping-keys";
const STORE_KEY_BYTES = 32;
const IV_BYTES = 12;
const LOCK_WAIT_MS = 2000;
const RETRY_MS = 50;

/// Holds the account's lock; resolves to a release function, or null when
/// another context still holds it after LOCK_WAIT_MS.
function holdLock(name) {
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), LOCK_WAIT_MS);
  return new Promise((resolve, reject) => {
    navigator.locks
      .request(name, { signal: abort.signal }, () => {
        clearTimeout(timer);
        return new Promise((release) => resolve(release));
      })
      .catch((error) => {
        clearTimeout(timer);
        if (error?.name === "AbortError") resolve(null);
        else reject(error);
      });
  });
}

/// Opens a sync access handle, retrying while a terminated worker's handle
/// on the same file is still being released.
async function openHandle(file, deadline) {
  for (;;) {
    try {
      return await file.createSyncAccessHandle();
    } catch (error) {
      if (error?.name !== "NoModificationAllowedError" || Date.now() >= deadline) throw error;
      await new Promise((r) => setTimeout(r, RETRY_MS));
    }
  }
}

function keyDatabase() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(KEY_DB, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(KEY_STORE);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

function transact(db, mode, run) {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(KEY_STORE, mode);
    const request = run(tx.objectStore(KEY_STORE));
    tx.oncomplete = () => resolve(request.result);
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error ?? new Error("the key transaction aborted"));
  });
}

async function readSealedKey(directory) {
  let handle;
  try {
    handle = await directory.getFileHandle(SEALED_KEY_FILE);
  } catch (error) {
    if (error?.name === "NotFoundError") return null;
    throw error;
  }
  const bytes = new Uint8Array(await (await handle.getFile()).arrayBuffer());
  return bytes.length === 0 ? null : bytes;
}

async function writeSealedKey(directory, bytes) {
  const handle = await directory.getFileHandle(SEALED_KEY_FILE, { create: true });
  const access = await handle.createSyncAccessHandle();
  try {
    access.truncate(0);
    access.write(bytes, { at: 0 });
    access.flush();
  } finally {
    access.close();
  }
}

async function logsHoldData(handles) {
  return handles.some((handle) => handle.getSize() > 0);
}

async function unseal(sealed, wrapping, aad) {
  if (!wrapping) {
    throw new Error(
      "this account's store key cannot be unsealed: its wrapping key is gone " +
        "(site data was partly cleared). The logs were left untouched.",
    );
  }
  try {
    const plain = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: sealed.subarray(0, IV_BYTES), additionalData: aad },
      wrapping,
      sealed.subarray(IV_BYTES),
    );
    return new Uint8Array(plain);
  } catch {
    throw new Error(
      "this account's store key does not unseal (its file is damaged, or its " +
        "wrapping key was replaced). The logs were left untouched.",
    );
  }
}

async function storeKey(directory, namespace, handles) {
  const db = await keyDatabase();
  try {
    let wrapping = await transact(db, "readonly", (store) => store.get(namespace));
    const sealed = await readSealedKey(directory);
    const aad = new TextEncoder().encode(`${ROOT}/${namespace}/store-key`);
    const holdsData = await logsHoldData(handles);
    if (sealed) {
      try {
        return await unseal(sealed, wrapping, aad);
      } catch (error) {
        // Empty logs hold no identity to strand: start the account afresh.
        if (holdsData) throw error;
      }
    } else if (holdsData) {
      throw new Error(
        "this account has logs but no sealed store key (site data was partly " +
          "cleared). The logs were left untouched.",
      );
    }
    if (!wrapping) {
      wrapping = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, [
        "encrypt",
        "decrypt",
      ]);
      await transact(db, "readwrite", (store) => store.put(wrapping, namespace));
    }
    const key = crypto.getRandomValues(new Uint8Array(STORE_KEY_BYTES));
    const iv = crypto.getRandomValues(new Uint8Array(IV_BYTES));
    const ciphertext = new Uint8Array(
      await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: aad }, wrapping, key),
    );
    const out = new Uint8Array(IV_BYTES + ciphertext.length);
    out.set(iv);
    out.set(ciphertext, IV_BYTES);
    // Durable before the first log record is sealed under it.
    await writeSealedKey(directory, out);
    return key;
  } finally {
    db.close();
  }
}

/// Opens one account's durable store. Resolves to `{ key, handles, close }`:
/// the 32-byte store key (the caller zeroes it once the engine has it), the
/// four log handles in the order `initializeDurable` takes them, and a
/// function that closes the handles and releases the lock.
export async function openDurableStore(namespace) {
  const release = await holdLock(`${ROOT}/${namespace}`);
  if (!release) {
    throw new Error("another node already runs this account in this browser");
  }
  const deadline = Date.now() + LOCK_WAIT_MS;
  const handles = [];
  const close = () => {
    for (const handle of handles) {
      try {
        handle.close();
      } catch {
        // Already closed, or the worker is ending; the browser releases it.
      }
    }
    handles.length = 0;
    release();
  };
  try {
    const root = await navigator.storage.getDirectory();
    const directory = await (
      await root.getDirectoryHandle(ROOT, { create: true })
    ).getDirectoryHandle(namespace, { create: true });
    for (const name of LOG_FILES) {
      const file = await directory.getFileHandle(name, { create: true });
      handles.push(await openHandle(file, deadline));
    }
    const key = await storeKey(directory, namespace, handles);
    return { key, handles: [...handles], close };
  } catch (error) {
    close();
    throw error;
  }
}
