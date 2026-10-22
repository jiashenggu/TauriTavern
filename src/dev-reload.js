export function installDevReload(url, { scope = globalThis, loadScript = () => import('./script.js') } = {}) {
    const source = new scope.EventSource(url);
    let active = false;
    let presentationPending = false;
    let pending = false;
    let checking = false;
    let disposed = false;
    let saveFailed = false;
    let timer = null;
    let reported = false;
    let announced = false;

    function locallyBusy() {
        const state = scope.__TAURITAVERN__?.api?.agent?.state;
        return active || presentationPending || scope.document?.body?.dataset?.generating === 'true'
            || state?.isSaving() || state?.needsReload();
    }

    async function attempt() {
        if (!pending || checking || disposed || saveFailed) return;
        if (timer !== null) scope.clearTimeout(timer);
        timer = null;
        checking = true;
        try {
            if (locallyBusy()) return;
            const agent = scope.__TAURITAVERN__?.api?.agent;
            if (scope.__TAURI_INTERNALS__ && !agent) return;
            let script;
            if (agent) {
                script = await loadScript();
                while (true) {
                    if (disposed || locallyBusy() || script.isGenerating() || script.isChatSaving) return;
                    const result = await agent.readActivity();
                    if (!Array.isArray(result?.activeRunIds)
                        || result.activeRunIds.some(id => typeof id !== 'string' || !id.trim())) {
                        throw new Error('Agent runtime activity could not be verified');
                    }
                    if (result.activeRunIds.length > 0) return;
                    if (disposed || locallyBusy() || script.isGenerating() || script.isChatSaving) return;
                    try {
                        // Debounced edits are not counted by isChatSaving yet.
                        // Drain them, then recheck activity and any edits queued
                        // while awaiting the save before deciding to navigate.
                        const save = script.flushDebouncedChatSave();
                        if (!save) break;
                        await save;
                    } catch (error) {
                        // Flushing consumes the pending task even on failure.
                        // A later empty queue cannot prove that data was saved.
                        saveFailed = true;
                        throw error;
                    }
                }
            }
            if (disposed || locallyBusy() || script?.isGenerating() || script?.isChatSaving) return;
            pending = false;
            scope.location.reload();
        } catch (error) {
            if (!reported || saveFailed) {
                scope.console.warn(saveFailed
                    ? '[Dev reload] Automatic reload stopped because a pending chat save failed. Save your changes successfully, then reload manually.'
                    : '[Dev reload] Reload deferred because safe idle state could not be verified.', error);
                reported = true;
            }
        } finally {
            checking = false;
            if (pending && !disposed && !saveFailed) {
                if (!announced) {
                    scope.console.info('[Dev reload] Changes queued; waiting for generation, saves and Agent publication to finish.');
                    announced = true;
                }
                timer = scope.setTimeout(attempt, 1000);
            }
        }
    }

    function requestReload() {
        pending = true;
        void attempt();
    }

    function runStateChanged(event) {
        active = Boolean(event.detail?.activeRun);
        presentationPending = Boolean(event.detail?.presentationError);
        void attempt();
    }

    source.addEventListener('reload', requestReload);
    scope.addEventListener('tauritavern-agent-run-state-changed', runStateChanged);
    return () => {
        disposed = true;
        if (timer !== null) scope.clearTimeout(timer);
        source.removeEventListener('reload', requestReload);
        scope.removeEventListener('tauritavern-agent-run-state-changed', runStateChanged);
        source.close();
    };
}
