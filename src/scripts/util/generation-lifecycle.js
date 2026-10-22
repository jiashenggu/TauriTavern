/**
 * Determines whether an unhandled Generate() error left the foreground
 * generation UI in a locked state that still needs the legacy unblock path.
 *
 * @param {{
 *   dryRun: boolean;
 *   isSendPress: boolean;
 *   isBodyGenerating: boolean;
 *   isGroupGenerating: boolean;
 * }} state
 * @returns {boolean}
 */
export function shouldUnblockGenerationAfterUnhandledError(state) {
    if (state.dryRun || state.isGroupGenerating) {
        return false;
    }

    return Boolean(state.isSendPress || state.isBodyGenerating);
}

/**
 * A completed request must deliver usable output. Reasoning and token usage are
 * not a reply; tool turns and image generation can legitimately have no text.
 *
 * @param {string} text Newly generated text, without a continuation's existing prefix.
 * @param {{ images?: string[], hasToolCalls?: boolean }} [options]
 */
export function assertGenerationOutput(text, { images = [], hasToolCalls = false } = {}) {
    if (text?.trim() || images.length || hasToolCalls) {
        return;
    }
    throw Object.assign(new Error('The model finished without returning reply text, images, or tool calls.'), {
        code: 'generation.empty_output',
    });
}

/**
 * Legacy character events describe visible Assistant content. A first-class
 * Assistant that only owns tool calls is published through the tool events.
 *
 * @param {ChatMessage} message
 * @param {boolean} hasToolCalls
 * @returns {boolean}
 */
export function shouldEmitCharacterMessageEvents(message, hasToolCalls) {
    if (!hasToolCalls) {
        return true;
    }

    const text = message.mes.trim();
    const reasoning = message.extra?.reasoning?.trim() ?? '';
    const hasMedia = Boolean(message.extra?.media?.length);
    return !['', '...'].includes(text) || Boolean(reasoning) || hasMedia;
}
