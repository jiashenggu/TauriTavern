import { invoke } from '../../../tauri-bridge.js';
import { stripJsonl } from '../../../tauri/main/kernel/chat-utils.js';
import {
    characterStemFromAvatarFileName,
    hasCharacterAvatarIdentity,
} from '../../../tauri/main/services/characters/character-identity.js';
import { createReadableFileStreamService } from '../../../tauri/main/services/files/readable-file-stream-service.js';
import { commitChatMetadata, commitChatPayload } from './commit.js';
import { jsonlStreamToPayload, jsonlToPayload } from './jsonl.js';
import { loadColdChatPayload } from './cold-swipes.js';

const { createReadableFileStream } = createReadableFileStreamService({ invoke });

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

function characterTarget({ characterName, avatarUrl, fileName }) {
    const characterId = resolveCharacterDirectoryId(characterName, avatarUrl);
    const normalizedFile = normalizeChatFileName(fileName);
    if (!characterId || !normalizedFile.trim()) {
        throw new Error('Invalid character chat target');
    }
    return { kind: 'character', characterId, fileName: normalizedFile };
}

function groupTarget(id) {
    const chatId = normalizeChatFileName(id);
    if (!chatId.trim()) {
        throw new Error('Invalid group chat target');
    }
    return { kind: 'group', chatId };
}

export async function loadCharacterChatPayload({ characterName, avatarUrl, fileName, allowNotFound = false, coldSwipes = false }) {
    const target = characterTarget({ characterName, avatarUrl, fileName });
    if (coldSwipes) return loadColdChatPayload(target, allowNotFound);
    const path = await invoke('get_chat_payload_path', {
        characterName: target.characterId,
        fileName: target.fileName,
        allowNotFound,
    });

    if (!path) {
        if (allowNotFound) {
            return [];
        }
        throw new Error('Chat payload path is empty');
    }

    const stream = createReadableFileStream(path);
    return jsonlStreamToPayload(stream);
}

export async function saveCharacterChatPayload({ characterName, avatarUrl, fileName, payload, force = false, commitReason = CHAT_COMMIT_REASON.MUTATION }) {
    await commitChatPayload({ target: characterTarget({ characterName, avatarUrl, fileName }), payload, force, commitReason });
}

export async function saveCharacterChatMetadata({ characterName, avatarUrl, fileName, chatMetadata }) {
    await commitChatMetadata({ target: characterTarget({ characterName, avatarUrl, fileName }), chatMetadata });
}

export async function loadCharacterChatPayloadBeforePages({ characterName, avatarUrl, fileName, cursor, maxLines, maxPages }) {
    const target = characterTarget({ characterName, avatarUrl, fileName });
    const result = await invoke('get_chat_payload_before_pages', {
        characterName: target.characterId,
        fileName: target.fileName,
        cursor,
        maxLines: Number(maxLines),
        maxPages: Number(maxPages),
    });

    if (!Array.isArray(result) || result.length === 0) {
        return [];
    }

    return result.map((page) => {
        const text = (page?.lines || []).join('\n');
        const messages = jsonlToPayload(text);
        return { messages, cursor: page.cursor, hasMoreBefore: Boolean(page.hasMoreBefore) };
    });
}

/**
 * Read character chat messages by absolute, zero-based message index.
 * Passing an empty index array returns the absolute total without loading message bodies.
 */
export async function readCharacterChatMessages({ characterName, avatarUrl, fileName, indices = [] }) {
    const target = characterTarget({ characterName, avatarUrl, fileName });
    if (!Array.isArray(indices)) {
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
        characterName: target.characterId,
        fileName: target.fileName,
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

export async function loadGroupChatPayload({ id, allowNotFound = false, coldSwipes = false }) {
    const target = groupTarget(id);
    if (coldSwipes) return loadColdChatPayload(target, allowNotFound);
    const path = await invoke('get_group_chat_path', { id: target.chatId, allowNotFound });

    if (!path) {
        if (allowNotFound) {
            return [];
        }
        throw new Error('Group chat payload path is empty');
    }

    const stream = createReadableFileStream(path);
    return jsonlStreamToPayload(stream);
}

export async function saveGroupChatPayload({ id, payload, force = false, commitReason = CHAT_COMMIT_REASON.MUTATION }) {
    await commitChatPayload({ target: groupTarget(id), payload, force, commitReason });
}

export async function saveGroupChatMetadata({ id, chatMetadata }) {
    await commitChatMetadata({ target: groupTarget(id), chatMetadata });
}
