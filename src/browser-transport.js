import { browserChannelTag, decodeWire, encodeWire, installBrowserRandomUUID } from './browser-wire.js';

export async function installBrowserTransport(target = window) {
    if (target.__TAURI__?.core?.invoke || target.__TAURI_INTERNALS__) {
        return;
    }
    installBrowserRandomUUID(target.crypto);
    // Capture native fetch before the existing /api interceptors are installed.
    const fetch = target.fetch.bind(target);
    const response = await fetch('/__remote/session', { cache: 'no-store' });
    if (!response.ok || (await response.json()).transport !== 1) {
        throw new Error('TauriTavern browser transport unavailable; sign in at /__remote/login');
    }
    const channels = new Map();
    const listeners = new Map();
    const events = new target.EventSource('/__remote/events');
    let disconnected = false;
    events.onmessage = ({ data }) => {
        const message = JSON.parse(data);
        if (message.kind === 'channel') {
            channels.get(message.id)?.onmessage(decodeWire(message.value));
        } else if (message.kind === 'event') {
            for (const callback of listeners.get(message.event) || []) {
                callback({ event: message.event, payload: message.payload });
            }
        }
    };
    await new Promise((resolve, reject) => {
        events.onopen = resolve;
        events.onerror = () => {
            events.close();
            reject(new Error('Could not connect to the desktop event transport'));
        };
    });
    events.onerror = () => {
        if (disconnected) return;
        disconnected = true;
        events.close();
        const notice = target.document.createElement('div');
        notice.setAttribute('role', 'alert');
        notice.style.cssText = 'position:fixed;top:0;left:0;right:0;z-index:2147483647;padding:1em;background:#8b2020;color:white';
        notice.textContent = 'Desktop connection lost. Unconfirmed writes are not retried. Reconnect and reload before continuing.';
        target.document.body.append(notice);
    };
    class Channel {
        [browserChannelTag] = true;
        id = target.crypto.randomUUID();
        constructor(onmessage = () => {}) {
            if (channels.size >= 256) {
                throw new Error('Browser channel limit reached; reload before creating another channel');
            }
            this.onmessage = onmessage;
            channels.set(this.id, this);
        }
    }
    async function invoke(command, args = {}, options = {}) {
        if (disconnected) {
            throw new Error('Desktop connection lost; reload before continuing');
        }
        // These are local browser behaviors, not native capabilities.
        if (command === 'plugin:opener|open_url') {
            const url = new URL(args.url);
            if (!['http:', 'https:', 'mailto:'].includes(url.protocol)) {
                throw new Error('Unsupported external URL');
            }
            target.open(url.href, '_blank', 'noopener,noreferrer');
            return;
        }
        if (command === 'backend_error_bridge_ready') {
            return []; // Do not drain the desktop UI's pending error queue.
        }
        // Only argument-free startup reads may be cached; keep invocation data out of URLs.
        const cacheableRead = ['get_bootstrap_snapshot', 'get_sillytavern_settings'].includes(command)
            && args && Object.keys(args).length === 0
            && Object.keys(options.headers || {}).length === 0;
        const response = await fetch(cacheableRead ? `/__remote/read/${command}` : '/__remote/invoke', cacheableRead ? {
            cache: 'no-cache',
        } : {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', 'X-TT-Remote': '1' },
            body: JSON.stringify({ command, args: encodeWire(args), headers: options.headers || {} }),
        });
        if (!response.ok) {
            throw new Error(await response.text());
        }
        const message = await response.json();
        if (!message.ok) {
            throw message.error ?? new Error('Desktop command failed');
        }
        return decodeWire(message.result);
    }
    target.__TAURITAVERN_BROWSER__ = true;
    target.__TAURI__ = {
        core: {
            invoke,
            Channel,
            convertFileSrc: (path, protocol = 'asset') => {
                if (protocol !== 'asset') {
                    throw new Error(`Native protocol ${protocol} is unavailable in a browser`);
                }
                return `/__remote/file?path=${encodeURIComponent(path)}`;
            },
        },
        event: {
            listen: async (event, callback) => {
                let callbacks = listeners.get(event);
                if (!callbacks) {
                    callbacks = new Set();
                    listeners.set(event, callbacks);
                }
                callbacks.add(callback);
                return () => {
                    callbacks.delete(callback);
                    if (callbacks.size === 0) listeners.delete(event);
                };
            },
        },
    };
    target.addEventListener('pagehide', () => {
        // Allow the host's other pagehide handlers to flush pending saves. There
        // is no interactive document again until pageshow, which fails closed.
        events.close();
        channels.clear();
        listeners.clear();
    }, { once: true });
    target.addEventListener('pageshow', (event) => {
        if (event.persisted) events.onerror();
    });
}
