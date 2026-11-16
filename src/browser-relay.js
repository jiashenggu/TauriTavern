import { decodeWire, encodeWire } from './browser-wire.js';

// This file runs ONLY in the dedicated local Tauri webview. It never evaluates
// browser-provided code, and its commands still pass normal Tauri IPC/ACL checks.
const { invoke, Channel } = window.__TAURI__.core;
await window.__TAURI__.event.listen('tt-browser-invoke', async ({ payload }) => {
    let value;
    try {
        const args = decodeWire(payload.args, (id) => new Channel((message) => {
            void invoke('browser_access_channel', { id, value: encodeWire(message) });
        }));
        const result = await invoke(payload.command, args, { headers: payload.headers || {} });
        value = { ok: true, result: encodeWire(result) ?? null };
    } catch (error) {
        value = { ok: false, error: error instanceof Error ? error.message : error };
    }
    await invoke('browser_access_reply', { id: payload.id, value });
});
await invoke('browser_access_ready');
