import assert from 'node:assert/strict';
import { test } from 'node:test';
import { webcrypto } from 'node:crypto';
import { runInNewContext } from 'node:vm';
import { decodeWire, encodeWire, installBrowserRandomUUID } from '../src/browser-wire.js';
import { installBrowserTransport } from '../src/browser-transport.js';

test('private HTTP UUID support uses secure random bytes and valid version/variant bits', () => {
    const crypto = { getRandomValues: webcrypto.getRandomValues.bind(webcrypto) };
    installBrowserRandomUUID(crypto);
    const first = crypto.randomUUID();
    assert.match(first, /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/);
    assert.notEqual(first, crypto.randomUUID());
    assert.throws(() => installBrowserRandomUUID({}), /secure random/);
});

test('chat byte responses and raw commit chunks round trip without changing byte ranges', () => {
    const backing = Uint8Array.from([20, 0, 127, 255, 30]);
    const payload = { chunk: backing.subarray(1, 4), rows: ['é', null, { message: 'synthetic chat' }] };
    const decoded = decodeWire(JSON.parse(JSON.stringify(encodeWire(payload))));
    assert.deepEqual(new Uint8Array(decoded.chunk), Uint8Array.from([0, 127, 255]));
    assert.deepEqual(decoded.rows, payload.rows);
    const bytes = new Uint8Array(4 * 1024 * 1024).fill(255);
    assert.deepEqual(new Uint8Array(decodeWire(encodeWire(bytes))), bytes);
    const reserved = { records: [{ __ttBinary: 'ordinary data' }, { __ttChannel: 'not a channel' }, { __ttObject: [['a', 1]] }] };
    assert.deepEqual(decodeWire(encodeWire(reserved)), reserved);
    const foreign = runInNewContext('Uint8Array.of(1, 255).buffer');
    assert.deepEqual(new Uint8Array(decodeWire(encodeWire(foreign))), Uint8Array.of(1, 255));
});

function browserHarness() {
    const requests = [];
    const handlers = new Map();
    let source;
    const target = {
        crypto: { getRandomValues: webcrypto.getRandomValues.bind(webcrypto) },
        EventSource: class {
            constructor() { source = this; queueMicrotask(() => this.onopen()); }
            close() { this.closed = true; }
        },
        fetch: async (url, options) => {
            if (url === '/__remote/session') return Response.json({ transport: 1 });
            const body = JSON.parse(options.body);
            requests.push(body);
            if (body.command === 'denied') return Response.json({ ok: false, error: 'desktop only' });
            return Response.json({ ok: true, result: encodeWire(Uint8Array.of(1, 2, 255)) });
        },
        addEventListener(name, callback) { handlers.set(name, callback); },
        document: { createElement: () => ({ setAttribute() {}, style: {} }), body: { append() {} } },
    };
    return { target, requests, source: () => source, handlers };
}

test('browser transport carries raw-save headers, byte results, channels, events and cancellation', async () => {
    const harness = browserHarness();
    await installBrowserTransport(harness.target);
    const { core, event } = harness.target.__TAURI__;
    const bytes = await core.invoke('append_chat_commit_chunk', Uint8Array.of(0, 255), {
        headers: { 'session-id': 'synthetic', offset: '0' },
    });
    assert.deepEqual(new Uint8Array(bytes), Uint8Array.of(1, 2, 255));
    assert.deepEqual(harness.requests[0].headers, { 'session-id': 'synthetic', offset: '0' });
    assert.deepEqual(new Uint8Array(decodeWire(harness.requests[0].args)), Uint8Array.of(0, 255));
    let chunk;
    const channel = new core.Channel((message) => { chunk = message; });
    await core.invoke('with-channel', { channel });
    const wireId = harness.requests[1].args.channel.__ttChannel;
    harness.source().onmessage({ data: JSON.stringify({ kind: 'channel', id: wireId, value: 'token' }) });
    assert.equal(chunk, 'token');
    let observed = 0;
    const unlisten = await event.listen('app-ready', () => observed++);
    const ready = { data: JSON.stringify({ kind: 'event', event: 'app-ready', payload: null }) };
    harness.source().onmessage(ready);
    unlisten();
    harness.source().onmessage(ready);
    assert.equal(observed, 1);
    await core.invoke('cancel_chat_completion_generation', { requestId: 'synthetic' });
    assert.equal(harness.requests.at(-1).command, 'cancel_chat_completion_generation');
    await assert.rejects(core.invoke('denied'), (error) => error === 'desktop only');
    harness.source().onerror();
    await assert.rejects(core.invoke('save_user_settings'), /connection lost/);
});

test('desktop transport is never replaced and performs no browser requests', async () => {
    const invoke = () => {};
    const target = { __TAURI__: { core: { invoke } }, fetch() { throw new Error('must not fetch'); } };
    await installBrowserTransport(target);
    assert.equal(target.__TAURI__.core.invoke, invoke);
});

test('a page restored from browser history cannot write with a closed event transport', async () => {
    const harness = browserHarness();
    await installBrowserTransport(harness.target);
    harness.handlers.get('pagehide')({ persisted: true });
    assert.equal(harness.source().closed, true);
    await harness.target.__TAURI__.core.invoke('save_user_settings');
    harness.handlers.get('pageshow')({ persisted: true });
    await assert.rejects(harness.target.__TAURI__.core.invoke('save_user_settings'), /connection lost/);
    assert.equal(harness.requests.length, 1, 'only the final pagehide flush may write');
});
