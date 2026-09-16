import { invoke } from '../../../tauri-bridge.js';
import { stripJsonl } from '../../../tauri/main/kernel/chat-utils.js';
import {
    characterStemFromAvatarFileName,
    hasCharacterAvatarIdentity,
} from '../../../tauri/main/services/characters/character-identity.js';
import { fetchAssetStream } from './asset-io.js';
import { commitChatPayload } from './commit.js';
import { jsonlStreamToPayload } from './jsonl.js';

export const CHAT_COMMIT_REASON = Object.freeze({
    MUTATION: 'mutation',
    PROVIDER_BARRIER: 'providerBarrier',
    GENERATION_CHECKPOINT: 'generationCheckpoint',
    MAINTENANCE: 'maintenance',
});

export function normalizeChatFileName(fileName) {
    return stripJsonl(fileName);
}

/**
 * Chat folders are keyed by the character avatar filename stem.
 * avatarUrl is SillyTavern's avatar_url API field, not a browser asset URL.
 */
export function resolveCharacterDirectoryId(characterName, avatarUrl) {
    if (hasCharacterAvatarIdentity(avatarUrl)) {
        return characterStemFromAvatarFileName(avatarUrl, 'avatar_url', { required: true });
    }

    return String(characterName || '').trim();
}

export async function loadCharacterChatPayload({ characterName, avatarUrl, fileName, allowNotFound = false }) {
    const normalizedCharacter = resolveCharacterDirectoryId(characterName, avatarUrl);
    const normalizedFile = normalizeChatFileName(fileName);
    if (!normalizedCharacter || !normalizedFile.trim()) {
        throw new Error('Invalid character chat payload request');
    }

    const path = await invoke('get_chat_payload_path', {
        characterName: normalizedCharacter,
        fileName: normalizedFile,
        allowNotFound,
    });

    if (!path) {
        if (allowNotFound) {
            return [];
        }
        throw new Error('Chat payload path is empty');
    }

    const stream = await fetchAssetStream(path);
    return jsonlStreamToPayload(stream);
}

/**
 * Read character chat messages by absolute, zero-based message index.
 * Passing an empty index array returns the absolute total without loading message bodies.
 */
export async function readCharacterChatMessages({ characterName, avatarUrl, fileName, indices = [] }) {
    const normalizedCharacter = resolveCharacterDirectoryId(characterName, avatarUrl);
    const normalizedFile = normalizeChatFileName(fileName);
    if (!normalizedCharacter || !normalizedFile.trim() || !Array.isArray(indices)) {
        throw new Error('Invalid absolute character chat message request');
    }

    const normalizedIndices = indices.map((index) => {
        const value = Number(index);
        if (!Number.isSafeInteger(value) || value < 0) {
            throw new Error(`Invalid absolute character chat message index: ${String(index)}`);
        }
        return value;
    });

    return invoke('read_character_chat_messages', {
        characterName: normalizedCharacter,
        fileName: normalizedFile,
        indices: normalizedIndices,
    });
}

/** Read an inclusive absolute message range from a character chat. */
export async function readCharacterChatMessageRange({ characterName, avatarUrl, fileName, start, end }) {
    const normalizedStart = Number(start);
    const normalizedEnd = Number(end);
    if (!Number.isSafeInteger(normalizedStart) || normalizedStart < 0
        || !Number.isSafeInteger(normalizedEnd) || normalizedEnd < normalizedStart) {
        throw new Error(`Invalid absolute character chat message range: ${String(start)}-${String(end)}`);
    }

    const indices = Array.from(
        { length: normalizedEnd - normalizedStart + 1 },
        (_, offset) => normalizedStart + offset,
    );
    return readCharacterChatMessages({ characterName, avatarUrl, fileName, indices });
}

export async function saveCharacterChatPayload({ characterName, avatarUrl, fileName, payload, force = false, commitReason = CHAT_COMMIT_REASON.MUTATION }) {
    const normalizedCharacter = resolveCharacterDirectoryId(characterName, avatarUrl);
    const normalizedFile = normalizeChatFileName(fileName);
    if (!Array.isArray(payload) || payload.length === 0 || !normalizedCharacter || !normalizedFile.trim()) {
        throw new Error('Invalid chat payload');
    }

    await commitChatPayload({
        target: {
            kind: 'character',
            characterId: normalizedCharacter,
            fileName: normalizedFile,
        },
        payload,
        force,
        commitReason,
    });
}

export async function loadGroupChatPayload({ id, allowNotFound = false }) {
    const normalizedId = normalizeChatFileName(id);
    if (!normalizedId.trim()) {
        throw new Error('Invalid group chat payload request');
    }

    const path = await invoke('get_group_chat_path', {
        id: normalizedId,
        allowNotFound,
    });

    if (!path) {
        if (allowNotFound) {
            return [];
        }
        throw new Error('Group chat payload path is empty');
    }

    const stream = await fetchAssetStream(path);
    return jsonlStreamToPayload(stream);
}

export async function saveGroupChatPayload({ id, payload, force = false, commitReason = CHAT_COMMIT_REASON.MUTATION }) {
    const normalizedId = normalizeChatFileName(id);
    if (!Array.isArray(payload) || payload.length === 0 || !normalizedId.trim()) {
        throw new Error('Invalid group chat payload');
    }

    await commitChatPayload({
        target: {
            kind: 'group',
            chatId: normalizedId,
        },
        payload,
        force,
        commitReason,
    });
}
