export function createShujukuTaskScopes(readContext) {
    let epoch = 0;
    const active = new Set();
    const aborted = () => Object.assign(new Error('[TableTask] 聊天或目标回复已变化，过期填表任务已取消 (aborted)。'), { name: 'AbortError', code: 'table_task_stale' });
    return {
        invalidate() {
            epoch++;
            for (const scope of active) scope.cancel();
        },
        capture(indices = [], parent = null) {
            parent?.assertCurrent();
            const original = { ...readContext() };
            const capturedEpoch = epoch;
            const controllers = new Set();
            let disposed = false;
            if (!Array.isArray(original.chat)) throw aborted();
            const targets = [...new Set(indices)].map(index => {
                const message = original.chat[index];
                if (!message || message.is_user) throw aborted();
                return { index, message, text: message.mes, swipe: message.swipe_id };
            });
            const scope = {
                assertCurrent() {
                    parent?.assertCurrent();
                    const current = readContext();
                    if (disposed || capturedEpoch !== epoch || current.chat !== original.chat
                        || current.chatId !== original.chatId || current.isolationKey !== original.isolationKey
                        || [...controllers].some(controller => controller.signal.aborted)
                        || targets.some(({ index, message, text, swipe }) => current.chat[index] !== message
                            || message.is_user || message.mes !== text || message.swipe_id !== swipe)) {
                        scope.cancel();
                        throw aborted();
                    }
                },
                track(controller) {
                    scope.assertCurrent();
                    controllers.add(controller);
                    scope.assertCurrent();
                    return () => controllers.delete(controller);
                },
                cancel() {
                    for (const controller of controllers) {
                        if (!controller.signal.aborted) controller.abort(aborted());
                    }
                },
                dispose() {
                    disposed = true;
                    controllers.clear();
                    active.delete(scope);
                },
            };
            scope.assertCurrent();
            active.add(scope);
            return scope;
        },
    };
}

export function patchShujukuLifecycle(source, createScopesSource = createShujukuTaskScopes.toString()) {
    source = source.replace(/\r\n/g, '\n');
    const replace = (text, before, after, expected = 1) => {
        const count = text.split(before).length - 1;
        if (count !== expected) throw new Error(`Unsupported shujuku build: expected ${expected} lifecycle anchor(s), got ${count}: ${before.slice(0, 90)}`);
        return text.split(before).join(after);
    };
    const update = (name, transform) => {
        const marker = `function ${name}(`;
        const at = source.indexOf(marker);
        if (at < 0 || source.indexOf(marker, at + marker.length) !== -1) throw new Error(`Unsupported shujuku function: ${name}`);
        const start = source.lastIndexOf('\n', at) + 1;
        const end = source.indexOf('\n    }\n', at) + 6;
        if (end < at) throw new Error(`Unterminated shujuku function: ${name}`);
        source = source.slice(0, start) + transform(source.slice(start, end)) + source.slice(end);
    };
    const wrap = (name, wrapper) => {
        const marker = `    async function ${name}(`;
        source = replace(source, marker, `${wrapper}\n    async function ${name}_Unscoped_TT(`);
    };
    const preamble = `\n    ${createScopesSource}\n    const tableTaskScopes_TT = createShujukuTaskScopes(() => ({ chat: getChatArray_ACU(), chatId: SillyTavern_API_ACU.getCurrentChatId(), isolationKey: getCurrentIsolationKey_ACU() }));\n`;
    source = replace(source, "(function () {\n    'use strict';", "(function () {\n    'use strict';" + preamble);
    source = replace(source,
        'SillyTavern_API_ACU.eventSource.on(SillyTavern_API_ACU.eventTypes.CHAT_CHANGED, async (chatFileName) => {',
        'SillyTavern_API_ACU.eventSource.on(SillyTavern_API_ACU.eventTypes.CHAT_CHANGED, async (chatFileName) => {\n                    tableTaskScopes_TT.invalidate();');
    source = replace(source,
        'SillyTavern_API_ACU.eventSource.on(SillyTavern_API_ACU.eventTypes[evName], async (data) => {',
        'SillyTavern_API_ACU.eventSource.on(SillyTavern_API_ACU.eventTypes[evName], async (data) => {\n                            tableTaskScopes_TT.invalidate();');

    update('processUpdatesBatch_ACU', text => {
        text = replace(text, '        if (!migration.success) {', '        options.taskScope.assertCurrent();\n        if (!migration.success) {');
        text = replace(text, '                const batchIndices = batches[i];', '                options.taskScope.assertCurrent();\n                const batchIndices = batches[i];');
        return replace(text, '                if (!baseResult.data) {', '                options.taskScope.assertCurrent();\n                if (!baseResult.data) {');
    });
    wrap('processUpdatesBatch_ACU', `    async function processUpdatesBatch_ACU(indicesToUpdate, mode, options = {}, executeUpdate) {
        let scope;
        try {
            scope = tableTaskScopes_TT.capture(indicesToUpdate || [], options.taskScope);
            return await processUpdatesBatch_ACU_Unscoped_TT(indicesToUpdate, mode, { ...options, taskScope: scope }, (...args) => {
                scope.assertCurrent();
                args[6] = { ...args[6], taskScope: scope };
                return executeUpdate(...args);
            });
        } catch (error) {
            if (error.name === 'AbortError') return { success: false, aborted: true, error: error.message };
            throw error;
        } finally { scope?.dispose(); }
    }`);

    update('executeCardUpdateCore_ACU', text => {
        text = replace(text, '                    const rawBaseSnapshot =', '                    progressContext.taskScope.assertCurrent();\n                    const rawBaseSnapshot =');
        text = replace(text, '                    if (collectResult.aborted) {', '                    progressContext.taskScope.assertCurrent();\n                    if (collectResult.aborted) {');
        text = replace(text, "                            reason: 'executeCardUpdateCore',", "                            reason: 'executeCardUpdateCore',\n                            taskScope: progressContext.taskScope,");
        text = replace(text, "                        reason: 'executeCardUpdateCore:snapshot',", "                        reason: 'executeCardUpdateCore:snapshot',\n                        taskScope: progressContext.taskScope,");
        text = replace(text, '                            const provider = await ensureStorageProviderReady_ACU();', '                            const provider = await ensureStorageProviderReady_ACU();\n                            progressContext.taskScope.assertCurrent();');
        text = replace(text, '                        if (!commitResult.success || !commitResult.value) {', '                        if (commitResult.aborted) return { success: false, modifiedKeys: [], aborted: true, error: commitResult.error };\n                        if (!commitResult.success || !commitResult.value) {');
        text = replace(text, '                    if (!updateOutcome.success || !updateOutcome.value) {', '                    if (updateOutcome.aborted) return { success: false, modifiedKeys: [], aborted: true, error: updateOutcome.error };\n                    if (!updateOutcome.success || !updateOutcome.value) {');
        return replace(text, '                    logWarn_ACU(`第 ${attempt} 次尝试失败: ${error.message}`);', "                    if (error.name === 'AbortError' || String(error.message || '').startsWith('[RuntimeRevision]')) return { success: false, modifiedKeys: [], aborted: true, error: error.message };\n                    logWarn_ACU(`第 ${attempt} 次尝试失败: ${error.message}`);");
    });
    wrap('executeCardUpdateCore_ACU', `    async function executeCardUpdateCore_ACU(messagesToUse, saveTargetIndex, isImportMode, updateMode, isSilentMode, targetSheetKeys, requestOptions, abortController, progressContext = null, onProgress) {
        if (typeof progressContext === 'function' && !onProgress) { onProgress = progressContext; progressContext = null; }
        let scope;
        try {
            const index = saveTargetIndex === -1 ? getChatArray_ACU().findLastIndex(message => !message.is_user) : saveTargetIndex;
            scope = tableTaskScopes_TT.capture(isImportMode ? [] : [index], progressContext?.taskScope);
            scope.track(abortController);
            const result = await executeCardUpdateCore_ACU_Unscoped_TT(messagesToUse, saveTargetIndex, isImportMode, updateMode, isSilentMode, targetSheetKeys, requestOptions, abortController, { ...progressContext, taskScope: scope }, onProgress);
            scope.assertCurrent();
            return result;
        } catch (error) {
            if (error.name === 'AbortError') return { success: false, modifiedKeys: [], aborted: true, error: error.message };
            throw error;
        } finally { scope?.dispose(); }
    }`);

    update('executeCardUpdateCore_ACU_Unscoped_TT', text => replace(text,
        '                        baseRevision,\n                        isImportMode,',
        '                        baseRevision,\n                        taskScope: progressContext.taskScope,\n                        isImportMode,'));
    update('collectGroupFillResponse_ACU', text => {
        text = replace(text, '        const dynamicContent = await prepareAIInput_ACU(', '        job.taskScope.assertCurrent();\n        const dynamicContent = await prepareAIInput_ACU(');
        text = replace(text, '        if (!dynamicContent) {', '        job.taskScope.assertCurrent();\n        if (!dynamicContent) {');
        text = replace(text, '            if (wasStoppedByUser_ACU$1) {', '            job.taskScope.assertCurrent();\n            if (wasStoppedByUser_ACU$1) {');
        return replace(text, '                if (abortController.signal.aborted || wasStoppedByUser_ACU$1) {', '                job.taskScope.assertCurrent();\n                if (abortController.signal.aborted || wasStoppedByUser_ACU$1) {');
    });
    update('processGroupedRuntimeChunk_ACU', text => {
        text = replace(text, '        if (!migration.success) {', '        options.taskScope.assertCurrent();\n        if (!migration.success) {');
        text = replace(text, '            const bucket = orderedBuckets[bucketIndex];', '            options.taskScope.assertCurrent();\n            const bucket = orderedBuckets[bucketIndex];');
        text = replace(text, '                if (!baseResult.data) {', '                options.taskScope.assertCurrent();\n                if (!baseResult.data) {');
        text = replace(text, '                        groupKey: plannedJob.group.key,', '                        taskScope: options.taskScope,\n                        groupKey: plannedJob.group.key,');
        text = replace(text, "                emitBucketProgress(bucketIndex, { phase: 'saving' });", "                options.taskScope.assertCurrent();\n                emitBucketProgress(bucketIndex, { phase: 'saving' });");
        text = replace(text, 'applySqlResponsesToCurrentRuntime_ACU(responses, baseSnapshot, bucket.updateMode)', 'applySqlResponsesToCurrentRuntime_ACU(responses, baseSnapshot, bucket.updateMode, options.taskScope)');
        text = replace(text, '                        saveTargetIndex: bucket.saveTargetIndex,', '                        taskScope: options.taskScope,\n                        saveTargetIndex: bucket.saveTargetIndex,');
        text = replace(text, '                if (applyResult.success) {', '                options.taskScope.assertCurrent();\n                if (applyResult.success) {');
        text = replace(text, "                            reason: 'manual_refill_progress_checkpoint',", "                            reason: 'manual_refill_progress_checkpoint',\n                            taskScope: options.taskScope,");
        text = replace(text, '                        if (!checkpointCommit.success) {', '                        options.taskScope.assertCurrent();\n                        if (!checkpointCommit.success) {');
        text = replace(text, '                            await updateReadableLorebookEntry_ACU(true, false, null, deferredCheckpointData);', '                            options.taskScope.assertCurrent();\n                            await updateReadableLorebookEntry_ACU(true, false, null, deferredCheckpointData);');
        return replace(text, "resetSqliteRuntimeFromSnapshot_ACU(deferredWorkingData, 'manual_refill_sql_runtime_init')", "resetSqliteRuntimeFromSnapshot_ACU(deferredWorkingData, 'manual_refill_sql_runtime_init', options.taskScope)");
    });
    wrap('processGroupedRuntimeChunk_ACU', `    async function processGroupedRuntimeChunk_ACU(groups, mode, options = {}) {
        let scope;
        try {
            scope = tableTaskScopes_TT.capture((groups || []).flatMap(group => group.indices || []), options.taskScope);
            const controller = options.abortController || new AbortController();
            scope.track(controller);
            const result = await processGroupedRuntimeChunk_ACU_Unscoped_TT(groups, mode, { ...options, taskScope: scope, abortController: controller });
            scope.assertCurrent();
            return result;
        } catch (error) {
            if (error.name === 'AbortError') return { success: false, aborted: true, failedGroups: (groups || []).map(group => group.key), error: error.message };
            throw error;
        } finally { scope?.dispose(); }
    }`);
    update('applySqlResponsesToCurrentRuntime_ACU', text => {
        text = replace(text, '(responses, baseSnapshot, updateMode) {', '(responses, baseSnapshot, updateMode, taskScope) {');
        return replace(text, '        const provider = await ensureStorageProviderReady_ACU();', '        taskScope?.assertCurrent();\n        const provider = await ensureStorageProviderReady_ACU();\n        taskScope?.assertCurrent();');
    });
    update('applyUnifiedGroupFillResponses_ACU', text => {
        text = replace(text, '        const sortedResponses = sortGroupFillResponses_ACU(responses);', '        options.taskScope?.assertCurrent();\n        const sortedResponses = sortGroupFillResponses_ACU(responses);');
        text = replace(text, "                source: 'group_fill',", "                source: 'group_fill',\n                taskScope: options.taskScope,", 2);
        return replace(text, '                const provider = await ensureStorageProviderReady_ACU();', '                const provider = await ensureStorageProviderReady_ACU();\n                options.taskScope?.assertCurrent();');
    });
    update('resetSqliteRuntimeFromSnapshot_ACU', text => {
        text = replace(text, '(snapshotData, reason) {', '(snapshotData, reason, taskScope) {');
        text = replace(text, '        const provider = await ensureStorageProviderReady_ACU();', '        taskScope?.assertCurrent();\n        const provider = await ensureStorageProviderReady_ACU();\n        taskScope?.assertCurrent();');
        return replace(text, 'provider.replaceAllData(snapshotData)', 'provider.replaceAllData(snapshotData, taskScope)');
    });
    source = replace(source,
        '        async replaceAllData(data) {\n            try {\n                const cloned = JSON.parse(JSON.stringify(data || {}));',
        '        async replaceAllData(data, taskScope) {\n            try {\n                taskScope?.assertCurrent();\n                const cloned = JSON.parse(JSON.stringify(data || {}));');
    source = replace(source,
        '                await this.engine.init();\n                this.syncBridge.loadFromTableData(cloned, { strict: true });',
        '                await this.engine.init();\n                taskScope?.assertCurrent();\n                this.syncBridge.loadFromTableData(cloned, { strict: true });');
    update('orchestrateManualUpdate_ACU', text => {
        text = replace(text, '                let pendingContextScopeIndices = contextScopeIndices.slice();', '                options.taskScope.assertCurrent();\n                let pendingContextScopeIndices = contextScopeIndices.slice();');
        return replace(text, "processGroupedRuntimeChunk_ACU(groupedChunk, 'manual_independent', {", "processGroupedRuntimeChunk_ACU(groupedChunk, 'manual_independent', {\n                    taskScope: options.taskScope,");
    });
    wrap('orchestrateManualUpdate_ACU', `    async function orchestrateManualUpdate_ACU(targetKeys, processBatch, refreshData, options = {}) {
        let scope;
        try {
            scope = tableTaskScopes_TT.capture([], options.taskScope);
            const guardedRefresh = async (...args) => {
                scope.assertCurrent();
                const result = await refreshData(...args);
                scope.assertCurrent();
                return result;
            };
            const result = await orchestrateManualUpdate_ACU_Unscoped_TT(targetKeys, processBatch, guardedRefresh, { ...options, taskScope: scope });
            scope.assertCurrent();
            return result;
        } catch (error) {
            if (error.name === 'AbortError') return { success: false, aborted: true, error: error.message };
            throw error;
        } finally { scope?.dispose(); }
    }`);

    update('runTableUpdateCommit_ACU', text => {
        text = replace(text, '        try {', '        try {\n            options.taskScope?.assertCurrent();');
        text = replace(text, '            if (!migration.success) {', '            options.taskScope?.assertCurrent();\n            if (!migration.success) {');
        text = replace(text, '\n                source: options.source,', '\n                source: options.source,\n                taskScope: options.taskScope,');
        text = replace(text, '                    const applied = await apply({ transactionContext, workingData });', '                    options.taskScope?.assertCurrent();\n                    const applied = await apply({ transactionContext, workingData });\n                    options.taskScope?.assertCurrent();');
        text = replace(text, '                        saved = saveResult.saved;', '                        options.taskScope?.assertCurrent();\n                        saved = saveResult.saved;');
        return replace(text, '            const message = error?.message || String(error);', "            if (error.name === 'AbortError') return { success: false, aborted: true, error: error.message };\n            const message = error?.message || String(error);");
    });
    update('runTableWriteTransaction_ACU', text => {
        text = replace(text, '        const chatKey =', '        options.taskScope?.assertCurrent();\n        const chatKey =');
        text = replace(text, '            const ctx = {', '            options.taskScope?.assertCurrent();\n            const ctx = {');
        text = replace(text, '                assertFresh: (reason) => {', '                assertFresh: (reason) => {\n                    options.taskScope?.assertCurrent();');
        text = replace(text, '                        assertRuntimeRevisionFresh_ACU(runtimeScopeKey, baseRevision, writeSet, options.reason);', '                        options.taskScope?.assertCurrent();\n                        assertRuntimeRevisionFresh_ACU(runtimeScopeKey, baseRevision, writeSet, options.reason);');
        return replace(text, '                        const result = await commitTask();', '                        const result = await commitTask();\n                        options.taskScope?.assertCurrent();');
    });
    wrap('executeAutoUpdatePlan_ACU', `    async function executeAutoUpdatePlan_ACU(plan, settings, setAutoUpdating, ops) {
        let scope;
        try {
            scope = tableTaskScopes_TT.capture(Object.values(plan.updateGroups || {}).flatMap(group => group.indices || []));
            const guarded = { ...ops };
            for (const key of ['processUpdates', 'processGroupedUpdates']) {
                if (typeof ops[key] === 'function') guarded[key] = (...args) => {
                    scope.assertCurrent();
                    args[2] = { ...args[2], taskScope: scope };
                    return ops[key](...args);
                };
            }
            for (const key of ['loadAllChatMessages', 'refreshData', 'purgeOldLayerData']) {
                if (typeof ops[key] === 'function') guarded[key] = async (...args) => {
                    scope.assertCurrent();
                    const value = await ops[key](...args);
                    scope.assertCurrent();
                    return value;
                };
            }
            return await executeAutoUpdatePlan_ACU_Unscoped_TT(plan, settings, setAutoUpdating, guarded);
        } catch (error) {
            if (error.name === 'AbortError') return { success: false, aborted: true, failedGroups: 0, totalGroups: Object.keys(plan.updateGroups || {}).length, errors: [error.message] };
            throw error;
        } finally { setAutoUpdating(false); scope?.dispose(); }
    }`);
    source = replace(source, '    function mainInitialize_ACU() {', "    function mainInitialize_ACU() {\n        globalThis.__SHUJUKU_LIFECYCLE_FIX__ = { version: 1, active: true };");
    return source;
}

export function createShujukuLifecycleLoader() {
    const url = 'https://gcore.jsdelivr.net/gh/AlbusKen/shujuku@spv7.1/index.js';
    return `const createShujukuTaskScopes = ${createShujukuTaskScopes.toString()};\nconst patchShujukuLifecycle = ${patchShujukuLifecycle.toString()};\nconst response = await fetch(${JSON.stringify(url)}, { credentials: 'omit' });\nif (!response.ok) throw new Error('Failed to load shujuku: HTTP ' + response.status);\nconst source = patchShujukuLifecycle(await response.text());\nconst moduleUrl = URL.createObjectURL(new Blob([source], { type: 'text/javascript' }));\ntry { await import(moduleUrl); } finally { URL.revokeObjectURL(moduleUrl); }\n`;
}
