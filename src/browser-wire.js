// The browser wire format is independent of Tauri. Only binary buffers and
// explicitly constructed channels have tags; ordinary DTOs remain JSON.
export const browserChannelTag = Symbol('TauriTavern browser channel');

export function encodeWire(value) {
    const isBuffer = Object.prototype.toString.call(value) === '[object ArrayBuffer]';
    if (isBuffer || ArrayBuffer.isView(value)) {
        const bytes = isBuffer
            ? new Uint8Array(value)
            : new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
        let binary = '';
        for (let offset = 0; offset < bytes.length; offset += 32768) {
            binary += String.fromCharCode(...bytes.subarray(offset, offset + 32768));
        }
        return { __ttBinary: btoa(binary) };
    }
    if (value?.[browserChannelTag] === true) {
        return { __ttChannel: value.id };
    }
    if (Array.isArray(value)) {
        return value.map(encodeWire);
    }
    if (value && typeof value === 'object') {
        const entries = Object.entries(value).map(([key, item]) => [key, encodeWire(item)]);
        if (entries.length === 1 && ['__ttBinary', '__ttChannel', '__ttObject'].includes(entries[0][0])) {
            return { __ttObject: entries }; // Escape ordinary user DTO keys.
        }
        return Object.fromEntries(entries);
    }
    return value;
}

export function decodeWire(value, createChannel) {
    if (value && typeof value === 'object' && Object.keys(value).length === 1) {
        if (Array.isArray(value.__ttObject)) {
            return Object.fromEntries(value.__ttObject.map(([key, item]) => [key, decodeWire(item, createChannel)]));
        }
        if (typeof value.__ttBinary === 'string') {
            return Uint8Array.from(atob(value.__ttBinary), (char) => char.charCodeAt(0)).buffer;
        }
        if (typeof value.__ttChannel === 'string') {
            if (!createChannel) {
                throw new Error('Unexpected browser channel response');
            }
            return createChannel(value.__ttChannel);
        }
    }
    if (Array.isArray(value)) {
        return value.map((item) => decodeWire(item, createChannel));
    }
    if (value && typeof value === 'object') {
        return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, decodeWire(item, createChannel)]));
    }
    return value;
}

// getRandomValues is available on private HTTP origins; randomUUID is not.
// No Math.random fallback: identifiers used by streams must remain unpredictable.
export function installBrowserRandomUUID(cryptoObject = globalThis.crypto) {
    if (typeof cryptoObject?.randomUUID === 'function') {
        return;
    }
    if (typeof cryptoObject?.getRandomValues !== 'function') {
        throw new Error('This browser does not provide secure random numbers');
    }
    cryptoObject.randomUUID = () => {
        const bytes = cryptoObject.getRandomValues(new Uint8Array(16));
        bytes[6] = (bytes[6] & 15) | 64;
        bytes[8] = (bytes[8] & 63) | 128;
        const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0'));
        return `${hex.slice(0, 4).join('')}-${hex.slice(4, 6).join('')}-${hex.slice(6, 8).join('')}-${hex.slice(8, 10).join('')}-${hex.slice(10).join('')}`;
    };
}
