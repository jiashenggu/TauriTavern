// @ts-check

import { registerLifecycleFlushHandler } from '../../services/lifecycle/lifecycle-flush-service.js';

// Keep script preferences in the existing account settings/save queue. The
// origin's real Storage remains a synchronous cache, including for card iframes.
const ACCOUNT_KEY = '__ttSharedLocalStorageV1';
const DEVICE_KEYS = new Set([
    'ai-context-config-state-v1',
    'floating-editor-state-v2',
    'floorImgMemZhV1',
    'statusBarImageCacheFailOriginV1',
]);
const DEVICE_KEY_PART = /(?:^|[.:_-])(?:pos|position|size|ball|lock|cache|journal|snapshots?|log|userInput|migrated|compatibility)(?:$|[.:_-])/i;

/** @param {string} key */
function isSharedKey(key) {
    return !DEVICE_KEYS.has(key)
        && !/^(?:tt:|tauritavern[.:])/.test(key)
        && !DEVICE_KEY_PART.test(key)
        && !/(?:EyePos|Position|Size|RadioLog)(?:$|[.:_-])/.test(key)
        && !/(?:球位|:last-selection)$/.test(key);
}

/** @param {Storage} storage @returns {Record<string, string>} */
function snapshot(storage) {
    const keys = [];
    for (let i = 0; i < storage.length; i++) {
        const key = storage.key(i);
        if (key !== null && isSharedKey(key)) keys.push(key);
    }
    return Object.fromEntries(keys.sort().map(key => [key, storage.getItem(key) ?? '']));
}

/** @param {string} raw @returns {Record<string, string>} */
function decode(raw) {
    const value = JSON.parse(raw);
    if (value?.version !== 1 || !value.entries || typeof value.entries !== 'object'
        || Array.isArray(value.entries)
        || Object.values(value.entries).some(entry => typeof entry !== 'string')) {
        throw new Error('Shared script settings are invalid; desktop settings must be repaired before continuing');
    }
    return Object.fromEntries(Object.entries(value.entries).filter(([key]) => isSharedKey(key)));
}

/** @type {(() => void) | undefined} */
let disposeBinding;

/**
 * Called after AccountStorage initialization and before extensions/cards load.
 * Only a native window may seed legacy values. Phone-origin defaults never
 * become the initial account settings.
 *
 * @param {{ getItem: (key: string) => string | null; setItem: (key: string, value: string) => void }} account
 */
export function installSharedLocalStorage(account) {
    const hostWindow = /** @type {any} */ (window);
    if (!hostWindow.__TAURI_RUNNING__) return;

    disposeBinding?.();
    disposeBinding = undefined;
    const storage = window.localStorage;
    const prototype = Object.getPrototypeOf(storage);
    const originalSet = prototype.setItem;
    const originalRemove = prototype.removeItem;
    const before = snapshot(storage);
    const saved = account.getItem(ACCOUNT_KEY);
    if (saved === null && hostWindow.__TAURITAVERN_BROWSER__) {
        throw new Error('Desktop script settings have not been initialized. Reload TauriTavern on the computer, then reload this page.');
    }
    const entries = saved === null ? before : decode(saved);

    /** @param {Record<string, string>} desired */
    const restore = (desired) => {
        for (const key of Object.keys(snapshot(storage))) {
            if (!Object.hasOwn(desired, key)) originalRemove.call(storage, key);
        }
        for (const [key, value] of Object.entries(desired)) {
            if (storage.getItem(key) !== value) originalSet.call(storage, key, value);
        }
    };
    try {
        restore(entries);
    } catch (error) {
        restore(before);
        throw error;
    }

    let active = true;
    const capture = () => {
        if (active) account.setItem(ACCOUNT_KEY, JSON.stringify({ version: 1, entries: snapshot(storage) }));
    };
    capture();

    // Own-window Storage writes do not dispatch a storage event. Preserve the
    // native API/exception semantics and ignore sessionStorage on this prototype.
    const unpatch = ['setItem', 'removeItem', 'clear'].map(name => {
        const original = prototype[name];
        /** @this {Storage} @param {...any} args */
        function write(...args) {
            const result = original.apply(this, args);
            if (this === storage) capture();
            return result;
        }
        prototype[name] = write;
        return () => {
            if (prototype[name] === write) prototype[name] = original;
        };
    });
    /** @param {StorageEvent} event */
    const onStorage = (event) => {
        if (event.storageArea === storage) capture();
    };
    window.addEventListener('storage', onStorage);
    // This also captures named-property writes before the existing settings
    // queue flushes on hide, unload, or graceful native exit.
    const unregister = registerLifecycleFlushHandler('shared-script-storage', capture, { priority: -101 });
    disposeBinding = () => {
        active = false;
        window.removeEventListener('storage', onStorage);
        unregister();
        unpatch.forEach(remove => remove());
    };
}
