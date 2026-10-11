import { resolveStableChatId, sameChatRef } from './agent-chat-identity.js';

export function createAgentStateApi({ safeInvoke, loadScript }) {
    let saving = false;
    let reloadRequired = false;

    function selection(script) {
        const chatRef = window.__TAURITAVERN__?.api?.chat?.current?.ref?.();
        if (!chatRef) throw new Error('agent.state_chat_missing: open a chat first');
        for (let index = script.chat.length - 1; index >= 0; index--) {
            const message = script.chat[index];
            if (message.is_user || message.is_system) continue;
            const metadata = message.extra?.tauritavern?.agent;
            if (!metadata) continue;
            if (metadata.persistStateStatus !== 'committed') {
                throw new Error('agent.persist_state_pending: finish or resume this reply before editing state');
            }
            return { chatRef, message, messageIndex: index, swipeId: message.swipe_id ?? null, stateId: metadata.persistStateId };
        }
        throw new Error('agent.state_missing: this chat has no published Agent state');
    }

    function assertSelection(script, captured) {
        const current = selection(script);
        if (!sameChatRef(current.chatRef, captured.chatRef)
            || current.message !== captured.message || current.swipeId !== captured.swipeId
            || current.stateId !== captured.stateId || current.messageIndex !== captured.messageIndex) {
            throw new Error('agent.state_conflict: chat or swipe changed; reload the state panel');
        }
    }

    function assertIdle() {
        if (reloadRequired) throw new Error('agent.state_save_unknown: reload the application before further chat changes');
        if (saving) throw new Error('agent.state_saving: wait for the state save to finish');
    }

    async function readCurrent() {
        assertIdle();
        const script = await loadScript();
        const captured = selection(script);
        const stableChatId = await resolveStableChatId(captured.chatRef);
        assertSelection(script, captured);
        const target = { chatRef: captured.chatRef, stableChatId, messageIndex: captured.messageIndex, swipeId: captured.swipeId };
        const document = await script.enqueueChatSave(() => {
            assertSelection(script, captured);
            return safeInvoke('read_agent_state_document', { dto: target });
        });
        assertSelection(script, captured);
        if (document.stateId !== captured.stateId) {
            throw new Error('agent.state_conflict: disk state differs from this chat view; reload the chat');
        }
        document.readOnly ||= script.isGenerating();
        return { target, document };
    }

    async function save(input) {
        assertIdle();
        saving = true;
        window.dispatchEvent(new CustomEvent('tauritavern-agent-state-saving'));
        try {
            const script = await loadScript();
            if (script.isGenerating()) throw new Error('agent.state_busy: generation is active');
            const captured = selection(script);
            if (!sameChatRef(captured.chatRef, input.target.chatRef)
                || captured.messageIndex !== input.target.messageIndex || captured.swipeId !== input.target.swipeId
                || captured.stateId !== input.expectedStateId) {
                throw new Error('agent.state_conflict: selected state changed; reload before saving');
            }
            const editId = crypto.randomUUID();
            return await script.enqueueChatSave(async () => {
                assertSelection(script, captured);
                if (script.isGenerating()) throw new Error('agent.state_busy: generation is active');
                let result;
                try {
                    result = await safeInvoke('save_agent_state_document', { dto: { ...input, editId } });
                } catch (error) {
                    if (/^(?:Bad request: |Conflict: )?agent\.(?:state_invalid|state_conflict):/u.test(String(error?.message ?? error))) throw error;
                    let saved;
                    try {
                        saved = await safeInvoke('read_agent_state_document', { dto: input.target });
                    } catch {
                        reloadRequired = true;
                        throw new Error('agent.state_save_unknown: save outcome cannot be confirmed; reload the application before continuing', { cause: error });
                    }
                    if (saved.manualEdit?.editId !== editId) throw error;
                    result = { stateId: saved.stateId, agentMetadata: { manualStateEdit: saved.manualEdit } };
                }
                const patch = extra => {
                    extra.tauritavern.agent.persistStateId = result.stateId;
                    extra.tauritavern.agent.manualStateEdit = result.agentMetadata.manualStateEdit;
                };
                if ((captured.message.swipe_id ?? null) === captured.swipeId) patch(captured.message.extra);
                const swipeExtra = captured.message.swipe_info?.[captured.swipeId]?.extra;
                if (swipeExtra) patch(swipeExtra);
                window.dispatchEvent(new CustomEvent('tauritavern-agent-state-changed', { detail: { target: input.target, stateId: result.stateId } }));
                return result;
            });
        } finally {
            saving = false;
            window.dispatchEvent(new CustomEvent('tauritavern-agent-state-saving'));
        }
    }

    return { readCurrent, save, isSaving: () => saving || reloadRequired, needsReload: () => reloadRequired, assertIdle };
}
