import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test } from '@rstest/core';

import { AgentSystemPanelApp } from './AgentSystemPanelApp';
import {
    createAgentSystemPanelController,
    type AgentSystemPanelController,
} from './AgentSystemPanelController';
import { createPanelWorld, settings, summary, tr } from './agent-system-panel-test-fixture';
import { createRunHistoryController } from './RunHistoryController';
import { createRunRetentionController } from './RunRetentionController';
import { defaultProfile } from './profile-model';
import type { AgentSystemSettings } from './settings-store';

const disposables: Array<{ dispose: () => void }> = [];

function renderPanel(controller: AgentSystemPanelController): void {
    const runHistory = createRunHistoryController({
        listRuns: () => Promise.resolve({ runs: [] }),
        currentChatRunFilter: () => Promise.resolve({
            chatRef: { kind: 'group', chatId: 'group' },
            stableChatId: 'stable-group',
        }),
        openRun: () => undefined,
        resumeRun: () => Promise.resolve(),
    });
    const runRetention = createRunRetentionController({
        getRetentionApi: () => ({
            readSettings: () => Promise.resolve({
                autoPruneEnabled: false,
                keepRecentTerminalRuns: 100,
                keepFullRecentRuns: 20,
            }),
            updateSettings: () => Promise.reject(new Error('unused')),
            planPrune: () => Promise.reject(new Error('unused')),
            applyPrune: () => Promise.reject(new Error('unused')),
        }),
        confirmAction: () => Promise.resolve(false),
        notifySuccess: () => undefined,
        notifyWarning: () => undefined,
        tr,
    });
    disposables.push(controller, runHistory, runRetention);
    render(
        <AgentSystemPanelApp
            controller={controller}
            runHistory={runHistory}
            runRetention={runRetention}
            tr={tr}
            onRequestClose={() => undefined}
        />,
    );
}

afterEach(() => {
    cleanup();
    disposables.splice(0).forEach((disposable) => disposable.dispose());
});

test('native connection bindings are editable without creating a Model Target or changing other profile settings', async () => {
    const profile = defaultProfile('scene-writer');
    profile.model = { mode: 'connectionRef', connectionRef: 'writer-low', modelId: 'original-model' };
    profile.run.directRunnable = false;
    profile.run.presentation = 'background';
    profile.delegation.callable = true;
    profile.delegation.allowAsSubagent = true;
    const { deps, state } = createPanelWorld(profile);
    deps.listLlmConnections = () => Promise.resolve([
        { id: 'writer-low', displayName: 'Writer Low', chatCompletionSource: 'custom' },
        { id: 'other-route', displayName: 'Other Route', chatCompletionSource: 'custom' },
    ]);
    deps.saveModelTargetConnection = () => Promise.reject(new Error('Native binding must not write a connection'));
    const controller = createAgentSystemPanelController(deps);
    const user = userEvent.setup();
    renderPanel(controller);
    await act(async () => controller.init());
    const connection = screen.getByRole<HTMLSelectElement>('combobox', { name: 'llmConnection' });
    const model = screen.getByRole<HTMLInputElement>('textbox', { name: 'modelId' });
    expect(connection.value).toBe('writer-low');
    expect(model.value).toBe('original-model');
    expect(screen.queryByText('agentProfileModelBindingMissing id=writer-low')).toBeNull();
    await user.selectOptions(connection, 'other-route');
    expect(model.value).toBe('original-model');
    await user.clear(model);
    await user.type(model, 'replacement-model');
    expect(state.saves).toHaveLength(0);
    await act(async () => controller.saveProfile());
    expect(state.saves).toHaveLength(1);
    expect(state.saves[0]?.model).toEqual({ mode: 'connectionRef', connectionRef: 'other-route', modelId: 'replacement-model' });
    const saved = structuredClone(state.saves[0]);
    if (!saved) throw new Error('profile not saved');
    saved.model = profile.model;
    expect(saved).toEqual(profile);
    expect(controller.getSnapshot().draft.model.modelId).toBe('replacement-model');
    await user.clear(screen.getByRole('textbox', { name: 'modelId' }));
    await act(async () => { await expect(controller.saveProfile()).rejects.toThrow('modelIdRequired'); });
    expect(state.saves).toHaveLength(1);
    await user.selectOptions(screen.getByRole('combobox', { name: 'modelSource' }), 'currentPromptSnapshot');
    await user.selectOptions(screen.getByRole('combobox', { name: 'modelSource' }), 'connectionRef');
    await user.selectOptions(screen.getByRole('combobox', { name: 'llmConnection' }), 'writer-low');
    expect(controller.getSnapshot().draft.model).toEqual({ mode: 'connectionRef', connectionRef: 'writer-low', modelId: '' });
});

test('connection directory failure, retry and deletion preserve a dirty model binding', async () => {
    const profile = defaultProfile('native-writer');
    profile.model = { mode: 'connectionRef', connectionRef: 'native', modelId: 'original' };
    const { deps, state } = createPanelWorld(profile);
    deps.listLlmConnections = () => Promise.reject(new Error('directory unavailable'));
    const controller = createAgentSystemPanelController(deps);
    const user = userEvent.setup();
    renderPanel(controller);
    await act(async () => controller.init());
    expect(screen.getByText('llmConnectionsUnavailable error=directory unavailable')).toBeDefined();
    expect(screen.getByRole<HTMLSelectElement>('combobox', { name: 'llmConnection' }).value).toBe('native');
    await user.clear(screen.getByRole('textbox', { name: 'modelId' }));
    await user.type(screen.getByRole('textbox', { name: 'modelId' }), 'unsaved-model');
    deps.listLlmConnections = () => Promise.resolve([{ id: 'native', displayName: 'Native', chatCompletionSource: 'custom' }]);
    await user.click(screen.getByRole('button', { name: 'refreshLlmConnections' }));
    await waitFor(() => expect(controller.getSnapshot().llmConnections).toHaveLength(1));
    expect(screen.queryByText('agentProfileModelBindingMissing id=native')).toBeNull();
    deps.listLlmConnections = () => Promise.resolve<TauriTavernLlmConnectionSummary[]>([]);
    act(() => state.llmConnectionsListener?.());
    await waitFor(() => expect(controller.getSnapshot().llmConnections).toHaveLength(0));
    expect(controller.getSnapshot().draft.model).toEqual({ mode: 'connectionRef', connectionRef: 'native', modelId: 'unsaved-model' });
    expect(screen.getByText('agentProfileModelBindingMissing id=native')).toBeDefined();
    expect(state.saves).toHaveLength(0);
});

test('saved Model Targets still fill both binding fields while a custom model ID survives save', async () => {
    const profile = defaultProfile('target-writer');
    const { deps, state } = createPanelWorld(profile);
    const target = { id: 'saved', name: 'Saved target', kind: 'tauritavern.modelTarget', mode: 'cc', api: 'custom', model: 'original', secretRef: { key: 'fixture', id: 'fixture' } };
    deps.listModelTargets = () => [target];
    const order: string[] = [];
    deps.saveModelTargetConnection = () => { order.push('connection'); return Promise.resolve(); };
    const save = deps.getProfilesApi().save;
    deps.getProfilesApi().save = input => { order.push('profile'); return save(input); };
    const controller = createAgentSystemPanelController(deps);
    const user = userEvent.setup();
    renderPanel(controller);
    await act(async () => controller.init());
    await user.selectOptions(screen.getByRole('combobox', { name: 'modelSource' }), 'connectionRef');
    expect(screen.getByRole<HTMLInputElement>('textbox', { name: 'modelId' }).value).toBe('original');
    await act(async () => controller.saveProfile());
    expect(order).toEqual(['connection', 'profile']);
    await user.clear(screen.getByRole('textbox', { name: 'modelId' }));
    await user.type(screen.getByRole('textbox', { name: 'modelId' }), 'override');
    expect(screen.getByRole<HTMLSelectElement>('combobox', { name: 'savedModel' }).value).toBe('');
    await act(async () => controller.saveProfile());
    expect(state.saves.at(-1)?.model).toEqual({ mode: 'connectionRef', connectionRef: 'model-target-saved', modelId: 'override' });
    await user.selectOptions(screen.getByRole('combobox', { name: 'savedModel' }), 'saved');
    expect(screen.getByRole<HTMLInputElement>('textbox', { name: 'modelId' }).value).toBe('original');
    await act(async () => controller.selectProfile('default-writer'));
    expect(screen.getByRole<HTMLSelectElement>('combobox', { name: 'modelSource' }).disabled).toBe(true);
});

test('late connection directory results cannot replace a newer catalog or update a disposed panel', async () => {
    const { deps } = createPanelWorld(defaultProfile('writer'));
    const controller = createAgentSystemPanelController(deps);
    disposables.push(controller);
    await controller.init();
    const pending: Array<(connections: TauriTavernLlmConnectionSummary[]) => void> = [];
    deps.listLlmConnections = () => new Promise(resolve => pending.push(resolve));
    const older = controller.refreshLlmConnections();
    const newer = controller.refreshLlmConnections();
    const latest = [{ id: 'latest', displayName: 'Latest', chatCompletionSource: 'custom' }];
    pending[1]?.(latest);
    await newer;
    pending[0]?.([{ id: 'old', displayName: 'Old', chatCompletionSource: 'custom' }]);
    await older;
    expect(controller.getSnapshot().llmConnections).toEqual(latest);
    const closing = controller.refreshLlmConnections();
    controller.dispose();
    pending[2]?.([]);
    await closing;
    expect(controller.getSnapshot().llmConnections).toEqual(latest);
});

test('repairs profile list issues and renders the refreshed list', async () => {
    const { deps, state } = createPanelWorld();
    state.listResults.push(
        {
            profiles: [summary(defaultProfile())],
            issues: [
                {
                    profileId: 'broken-json',
                    fileName: 'broken-json.json',
                    kind: 'invalidJson',
                    recommendedAction: 'delete',
                    message: 'Invalid JSON',
                },
                {
                    profileId: 'bad-schema',
                    fileName: 'bad-schema.json',
                    kind: 'invalidFileIdentity',
                    recommendedAction: 'normalizeIdentity',
                    message: 'Invalid profile kind',
                },
            ],
        },
        {
            profiles: [summary(defaultProfile()), {
                id: 'bad-schema',
                displayName: 'bad-schema',
                directRunnable: true,
            }],
            issues: [],
        },
    );
    const controller = createAgentSystemPanelController(deps);
    renderPanel(controller);

    await act(async () => controller.init());

    expect(state.repairs).toEqual([
        { profileId: 'broken-json', action: 'delete' },
        { profileId: 'bad-schema', action: 'normalizeIdentity' },
    ]);
    expect(state.confirmations).toHaveLength(1);
    expect(state.warnings).toEqual([
        'deletedCorruptAgentProfile id=broken-json',
        'normalizedAgentProfileIdentity id=bad-schema',
    ]);
    expect(screen.getAllByText('bad-schema').length).toBeGreaterThan(0);
});

test('keeps a profile editable when its prompt preview fails', async () => {
    const profile = defaultProfile('dangling-writer');
    profile.displayName = 'Dangling Writer';
    profile.preset = {
        mode: 'ref',
        ref: { apiId: 'openai', name: 'Missing Writer Preset' },
        required: true,
    };
    const { deps, state } = createPanelWorld(profile);
    state.presetOptions = ['Missing Writer Preset'];
    state.resolveError = new Error('agent.profile_preset_missing');
    state.health = {
        profileId: profile.id,
        previewAvailable: true,
        promptAssemblyAvailable: false,
        directRunAvailable: false,
        subAgentAvailable: false,
        diagnostics: [{
            code: 'agent.profile_preset_missing',
            severity: 'error',
            path: '$.preset.ref.name',
            message: 'required preset is missing',
            resource: { kind: 'preset', apiId: 'openai', name: 'Missing Writer Preset' },
            blocks: ['promptAssembly', 'directRun', 'subAgent'],
            repairActions: ['selectPreset'],
        }],
    };
    const controller = createAgentSystemPanelController(deps);
    const user = userEvent.setup();
    renderPanel(controller);
    await act(async () => controller.init());

    await waitFor(() => expect(screen.getByText('agentProfilePresetMissing name=Missing Writer Preset')).toBeDefined());
    const displayName = screen.getByRole<HTMLInputElement>('textbox', { name: 'displayName' });
    await user.clear(displayName);
    await user.type(displayName, 'Editable Writer');

    expect(displayName.value).toBe('Editable Writer');
    expect(controller.getSnapshot().profilePreviewError).toBe('agent.profile_preset_missing');
});

test('supplemental catalog failures do not block profile editing', async () => {
    const profile = defaultProfile('writer');
    const { deps, state } = createPanelWorld(profile);
    deps.listPresetOptions = () => { throw new Error('preset catalog unavailable'); };
    deps.listModelTargets = () => { throw new Error('model catalog unavailable'); };
    deps.listTools = () => Promise.reject(new Error('tool catalog unavailable'));
    const controller = createAgentSystemPanelController(deps);
    const user = userEvent.setup();
    renderPanel(controller);

    await act(async () => controller.init());

    expect(controller.getSnapshot().initialized).toBe(true);
    expect(state.subscribers).toEqual({ profiles: 1, modelTargets: 1, llmConnections: 1 });
    expect(state.errors).toEqual(expect.arrayContaining([
        'preset catalog unavailable',
        'model catalog unavailable',
        'tool catalog unavailable',
    ]));
    const displayName = screen.getByRole<HTMLInputElement>('textbox', { name: 'displayName' });
    await user.clear(displayName);
    await user.type(displayName, 'Still editable');
    expect(displayName.value).toBe('Still editable');
});

test('tool catalog keeps available disabled tools visible', async () => {
    const profile = defaultProfile('writer');
    profile.tools.allow = [];
    const { deps } = createPanelWorld(profile);
    deps.listTools = () => Promise.resolve({
        tools: [{
            id: 'builtin:workspace.read_file',
            nativeName: 'workspace.read_file',
            title: 'Read workspace file',
            description: 'Reads a workspace file.',
            inputSchema: {},
            source: 'builtin',
        }],
        diagnostics: [],
    });
    const controller = createAgentSystemPanelController(deps);
    renderPanel(controller);

    await act(async () => controller.init());

    expect(screen.getAllByText('Read workspace file').length).toBeGreaterThan(0);
});

test('failed profile selection keeps the previous selection and draft together', async () => {
    const writer = defaultProfile('writer');
    const reviewer = defaultProfile('reviewer');
    reviewer.displayName = 'Reviewer';
    const { deps, state } = createPanelWorld(writer);
    state.definitions.set(reviewer.id, reviewer);
    const patchSettings = deps.patchSettings;
    deps.patchSettings = (current, patch) => patch.editingProfileId === reviewer.id
        ? Promise.reject(new Error('settings write failed'))
        : patchSettings(current, patch);
    const controller = createAgentSystemPanelController(deps);
    disposables.push(controller);
    await controller.init();

    await expect(controller.selectProfile(reviewer.id)).rejects.toThrow('settings write failed');

    expect(controller.getSnapshot().editingProfileId).toBe(writer.id);
    expect(controller.getSnapshot().draft.id).toBe(writer.id);
    expect(state.errors).toContain('settings write failed');
});

test('a saved profile remains committed when its list refresh fails', async () => {
    const profile = defaultProfile('writer');
    const { deps, state } = createPanelWorld(profile);
    const controller = createAgentSystemPanelController(deps);
    disposables.push(controller);
    await controller.init();
    deps.getProfilesApi().list = () => Promise.reject(new Error('profile list refresh failed'));
    controller.setIdentityField('displayName', 'Saved Writer');

    await controller.saveProfile();

    expect(state.definitions.get(profile.id)?.displayName).toBe('Saved Writer');
    expect(controller.getSnapshot().draft.displayName).toBe('Saved Writer');
    expect(state.errors).toContain('profile list refresh failed');
});

test('a dirty draft rejects an external overwrite and save', async () => {
    const profile = defaultProfile('writer');
    profile.preset = {
        mode: 'ref',
        ref: { apiId: 'openai', name: 'Old Preset' },
        required: true,
    };
    const { deps, state } = createPanelWorld(profile);
    state.presetOptions = ['Old Preset', 'New Preset'];
    const controller = createAgentSystemPanelController(deps);
    const user = userEvent.setup();
    renderPanel(controller);
    await act(async () => controller.init());

    const displayName = screen.getByRole<HTMLInputElement>('textbox', { name: 'displayName' });
    await user.clear(displayName);
    await user.type(displayName, 'Unsaved local edit');
    const changed = structuredClone(profile);
    if (changed.preset.mode !== 'ref' || !changed.preset.ref) {
        throw new Error('expected a preset reference');
    }
    changed.preset.ref.name = 'New Preset';
    state.definitions.set(profile.id, changed);

    act(() => state.profilesListener?.());
    await waitFor(() => expect(controller.getSnapshot().externalProfileChangePending).toBe(true));
    act(() => state.profilesListener?.());
    await waitFor(() => expect(state.listCount).toBe(3));

    expect(state.warnings).toEqual(['agentProfileExternalChangePending']);
    await expect(controller.saveProfile()).rejects.toThrow('agentProfileExternalChangeSaveBlocked');
    expect(state.saves).toEqual([]);
});

test('dispose and failed subscription setup leave no ghost listeners', async () => {
    const pending = createPanelWorld();
    const deferredSettings: { resolve: ((value: AgentSystemSettings) => void) | null } = { resolve: null };
    pending.deps.loadSettings = () => new Promise((resolve) => {
        deferredSettings.resolve = resolve;
    });
    const pendingController = createAgentSystemPanelController(pending.deps);
    disposables.push(pendingController);
    const initPromise = pendingController.init();
    await waitFor(() => expect(deferredSettings.resolve).not.toBeNull());
    pendingController.dispose();
    deferredSettings.resolve?.(settings('default-writer'));
    await initPromise;

    expect(pending.state.subscribers).toEqual({ profiles: 0, modelTargets: 0, llmConnections: 0 });
    expect(pendingController.getSnapshot().initialized).toBe(false);

    const failed = createPanelWorld();
    failed.deps.subscribeLlmConnectionsChanged = () => {
        throw new Error('LLM subscription failed');
    };
    const failedController = createAgentSystemPanelController(failed.deps);
    disposables.push(failedController);

    await expect(failedController.init()).rejects.toThrow('LLM subscription failed');
    expect(failed.state.subscribers).toEqual({ profiles: 0, modelTargets: 0, llmConnections: 0 });
    expect(failedController.getSnapshot().initialized).toBe(false);
});
