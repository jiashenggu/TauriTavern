import type { AgentSystemPanelControllerDeps } from './AgentSystemPanelContract';
import { defaultProfile } from './profile-model';
import type { AgentSystemSettings } from './settings-store';

type ProfileListResult = Awaited<ReturnType<TauriTavernAgentProfilesApi['list']>>;

function formatParam(value: unknown): string {
    if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') return String(value);
    return JSON.stringify(value) ?? '';
}

export const tr = (key: string, params: Record<string, unknown> = {}): string => (
    [key, ...Object.entries(params).map(([name, value]) => `${name}=${formatParam(value)}`)].join(' ')
);

export function settings(editingProfileId: string): AgentSystemSettings {
    return {
        agentModeEnabled: true, chatInputToggleHidden: false, activeProfileId: 'default-writer',
        editingProfileId, activeTab: 'profiles', runTimelineHeightPx: null,
    };
}

export function summary(profile: TauriTavernAgentProfileDefinition): TauriTavernAgentProfileSummary {
    return {
        id: profile.id, displayName: profile.displayName,
        ...(profile.description !== undefined ? { description: profile.description } : {}),
        directRunnable: profile.run.directRunnable,
    };
}

function healthyProfile(profileId: string): TauriTavernAgentProfileHealth {
    return {
        profileId, previewAvailable: true, promptAssemblyAvailable: true,
        directRunAvailable: true, subAgentAvailable: true, diagnostics: [],
    };
}

export function createPanelWorld(selectedProfile = defaultProfile()) {
    const definitions = new Map<string, TauriTavernAgentProfileDefinition>();
    const builtin = defaultProfile();
    definitions.set(builtin.id, builtin);
    definitions.set(selectedProfile.id, selectedProfile);
    const state = {
        settings: settings(selectedProfile.id), definitions,
        listResults: [] as ProfileListResult[], listCount: 0,
        repairs: [] as Array<{ profileId: string; action: 'delete' | 'normalizeIdentity' }>,
        confirmations: [] as string[], confirm: true,
        warnings: [] as string[], errors: [] as string[], saves: [] as TauriTavernAgentProfileDefinition[],
        presetOptions: [] as string[], health: healthyProfile(selectedProfile.id), resolveError: null as Error | null,
        profilesListener: null as (() => void) | null,
        llmConnectionsListener: null as (() => void) | null,
        subscribers: { profiles: 0, modelTargets: 0, llmConnections: 0 },
    };
    const profilesApi: TauriTavernAgentProfilesApi = {
        list: () => {
            state.listCount += 1;
            return Promise.resolve(state.listResults.shift() ?? { profiles: [...state.definitions.values()].map(summary), issues: [] });
        },
        load: input => {
            const profile = state.definitions.get(typeof input === 'string' ? input : input.profileId);
            return Promise.resolve({ profile: profile ? structuredClone(profile) : null });
        },
        diagnose: () => Promise.resolve(state.health),
        resolveSystemPrompt: () => state.resolveError
            ? Promise.reject(state.resolveError)
            : Promise.resolve({ agentSystemPrompt: 'Resolved Agent system prompt.' }),
        repairFile: input => { state.repairs.push(input); return Promise.resolve(); },
        retargetPresetRefs: () => Promise.resolve({ updated: 0, profileIds: [] }),
        save: input => {
            const profile = 'profile' in input ? input.profile : input;
            state.saves.push(profile);
            state.definitions.set(profile.id, structuredClone(profile));
            return Promise.resolve();
        },
        delete: input => {
            state.definitions.delete(typeof input === 'string' ? input : input.profileId);
            return Promise.resolve();
        },
    };
    const deps: AgentSystemPanelControllerDeps = {
        loadSettings: () => Promise.resolve(state.settings),
        patchSettings: (current, patch) => {
            state.settings = { ...current, ...patch };
            return Promise.resolve(state.settings);
        },
        getProfilesApi: () => profilesApi,
        listTools: () => Promise.resolve({ tools: [], diagnostics: [] }),
        listPresetOptions: () => state.presetOptions,
        listModelTargets: () => [],
        listLlmConnections: () => Promise.resolve([]),
        saveModelTargetConnection: () => Promise.resolve(),
        subscribeProfilesChanged: listener => {
            state.subscribers.profiles += 1;
            state.profilesListener = listener;
            return () => { state.subscribers.profiles -= 1; };
        },
        subscribeModelTargetsChanged: () => {
            state.subscribers.modelTargets += 1;
            return () => { state.subscribers.modelTargets -= 1; };
        },
        subscribeLlmConnectionsChanged: listener => {
            state.subscribers.llmConnections += 1;
            state.llmConnectionsListener = listener;
            return () => { state.subscribers.llmConnections -= 1; state.llmConnectionsListener = null; };
        },
        confirmAction: message => { state.confirmations.push(message); return Promise.resolve(state.confirm); },
        downloadBlob: () => Promise.resolve({ mode: 'browser-download', completed: true }),
        notifyError: error => { state.errors.push(error instanceof Error ? error.message : 'unknown error'); },
        notifyWarning: message => { state.warnings.push(message); },
        notifySuccess: () => undefined,
        onRunsTabActivated: () => undefined,
        tr,
    };
    return { deps, state };
}
