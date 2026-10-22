import type { AgentSystemPanelController } from './AgentSystemPanelController';
import { isBuiltinProfile, type AgentSystemPanelSnapshot, type Tr } from './AgentSystemPanelContract';
import {
    availablePresetOptions,
    isProfileRuntimeStateCurrent,
    modelSummaryLabel,
    modelTargetBadges,
    presetSummaryLabel,
    profileConfigurationWarnings,
    selectedModelTarget,
} from './AgentSystemPanelView';

export type ProfileSectionProps = {
    snapshot: AgentSystemPanelSnapshot;
    controller: AgentSystemPanelController;
    tr: Tr;
};

export function ProfileIdentitySection({ snapshot, controller, tr }: ProfileSectionProps) {
    const { draft } = snapshot;
    const builtin = isBuiltinProfile(draft);
    return (
        <div className="ttas-section" data-ttas-profile-section="identity">
            <div className="ttas-section-title">
                <i className="fa-solid fa-fingerprint"></i>
                <h4>{tr('identity')}</h4>
            </div>
            <div className="ttas-form-grid">
                <label className="ttas-field">
                    <span>{tr('profileId')}</span>
                    <input
                        className="text_pole"
                        value={draft.id}
                        disabled={builtin}
                        onChange={(event) => controller.setIdentityField('id', event.target.value)}
                    />
                </label>
                <label className="ttas-field">
                    <span>{tr('displayName')}</span>
                    <input
                        className="text_pole"
                        value={draft.displayName}
                        disabled={builtin}
                        onChange={(event) => controller.setIdentityField('displayName', event.target.value)}
                    />
                </label>
                <label className="ttas-field ttas-span-2">
                    <span>{tr('description')}</span>
                    <input
                        className="text_pole"
                        value={draft.description ?? ''}
                        disabled={builtin}
                        onChange={(event) => controller.setIdentityField('description', event.target.value)}
                    />
                </label>
            </div>
        </div>
    );
}

export function ProfileBindingSection({ snapshot, controller, tr }: ProfileSectionProps) {
    const { draft, presetOptions, modelTargets } = snapshot;
    const builtin = isBuiltinProfile(draft);
    const presetOptionsWithSelected = availablePresetOptions(presetOptions, draft);
    const selectedTarget = selectedModelTarget(modelTargets, draft);
    const selectedTargetId = selectedTarget?.id || '';
    const connectionRef = draft.model.connectionRef || '';
    const connections = snapshot.llmConnections ?? [];
    const selectedConnection = connections.find(connection => connection.id === connectionRef);
    const warnings = profileConfigurationWarnings({
        draft,
        toolCatalogDiagnostics: snapshot.toolCatalogDiagnostics,
        profileHealth: snapshot.profileHealth,
        isRuntimeStateCurrent: isProfileRuntimeStateCurrent(draft, snapshot.profileRuntimeStateJson),
        profileDiagnosticError: snapshot.profileDiagnosticError,
        profilePreviewError: snapshot.profilePreviewError,
        presetOptions,
        modelTargets,
        llmConnections: snapshot.llmConnections,
    }, tr);

    return (
        <div className="ttas-section" data-ttas-profile-section="binding">
            <div className="ttas-section-title">
                <i className="fa-solid fa-sliders"></i>
                <h4>{tr('presetAndModel')}</h4>
            </div>
            <div className="ttas-form-grid">
                <label className="ttas-field">
                    <span>{tr('presetSource')}</span>
                    <select
                        value={draft.preset.mode}
                        disabled={builtin}
                        onChange={(event) => controller.setPresetMode(event.target.value)}
                    >
                        <option value="currentPromptSnapshot">{tr('currentPromptPreset')}</option>
                        <option value="ref">{tr('savedChatCompletionPreset')}</option>
                        <option value="none">{tr('noPromptPreset')}</option>
                    </select>
                </label>
                {draft.preset.mode === 'ref' ? (
                    <label className="ttas-field">
                        <span>{tr('savedPreset')}</span>
                        <select
                            value={draft.preset.ref?.name || ''}
                            disabled={builtin}
                            onChange={(event) => controller.setPresetName(event.target.value)}
                        >
                            {presetOptionsWithSelected.length === 0 && <option value="">{tr('none')}</option>}
                            {presetOptionsWithSelected.map((name) => (
                                <option key={name} value={name}>{name}</option>
                            ))}
                        </select>
                    </label>
                ) : (
                    <div className="ttas-binding-status">
                        <i className="fa-solid fa-scroll"></i>
                        <strong>{presetSummaryLabel(draft, tr)}</strong>
                        <span>{tr('preset')}</span>
                    </div>
                )}

                <label className="ttas-field">
                    <span>{tr('modelSource')}</span>
                    <select
                        value={draft.model.mode}
                        disabled={builtin || snapshot.saving}
                        onChange={(event) => controller.setModelMode(event.target.value)}
                    >
                        <option value="currentPromptSnapshot">{tr('currentChatModel')}</option>
                        {draft.model.mode === 'requiresConfiguration' && (
                            <option value="requiresConfiguration">{tr('modelRequiresConfiguration')}</option>
                        )}
                        <option value="connectionRef">{tr('independentModel')}</option>
                    </select>
                </label>
                {draft.model.mode === 'connectionRef' && modelTargets.length > 0 ? (
                    <label className="ttas-field">
                        <span>{tr('savedModel')}</span>
                        <select
                            value={selectedTargetId}
                            disabled={builtin || snapshot.saving}
                            onChange={(event) => controller.setModelTarget(event.target.value)}
                        >
                            <option value="" disabled>{tr('chooseSavedModel')}</option>
                            {modelTargets.map((target) => (
                                <option key={target.id} value={target.id}>{target.name || target.model}</option>
                            ))}
                        </select>
                    </label>
                ) : (
                    <div className="ttas-binding-status">
                        <i className="fa-solid fa-microchip"></i>
                        <strong>{modelSummaryLabel(draft, modelTargets, tr)}</strong>
                        <span>{tr('model')}</span>
                    </div>
                )}

                {selectedTarget && (
                    <div className="ttas-binding-summary ttas-span-2">
                        <i className="fa-solid fa-plug-circle-check"></i>
                        <div>
                            <strong>{selectedTarget.name || selectedTarget.model}</strong>
                            <div className="ttas-tool-badge-row">
                                {modelTargetBadges(selectedTarget).map((badge) => (
                                    <span key={badge}>{badge}</span>
                                ))}
                            </div>
                        </div>
                    </div>
                )}
                {draft.model.mode === 'connectionRef' && (
                    <>
                        <label className="ttas-field">
                            <span>{tr('llmConnection')}</span>
                            <select
                                value={connectionRef}
                                disabled={builtin || snapshot.saving}
                                onChange={event => controller.setModelConnection(event.target.value)}
                            >
                                <option value="" disabled>{tr('selectLlmConnection')}</option>
                                {connectionRef && !selectedConnection && (
                                    <option value={connectionRef} disabled>{tr('currentUnlistedConnection', { id: connectionRef })}</option>
                                )}
                                {connections.map(connection => (
                                    <option key={connection.id} value={connection.id}>{connection.displayName} ({connection.id})</option>
                                ))}
                            </select>
                        </label>
                        <label className="ttas-field">
                            <span>{tr('modelId')}</span>
                            <input
                                className="text_pole"
                                value={draft.model.modelId || ''}
                                disabled={builtin || snapshot.saving}
                                required
                                onChange={event => controller.setModelId(event.target.value)}
                            />
                        </label>
                        <div className="ttas-binding-summary ttas-span-2">
                            <div>
                                {selectedConnection && <strong>{selectedConnection.chatCompletionSource}{selectedConnection.customApiFormat ? ` / ${selectedConnection.customApiFormat}` : ''}</strong>}
                                <span>{tr('modelBindingHint')}</span>
                                {snapshot.llmConnectionsLoading && <span role="status">{tr('loadingLlmConnections')}</span>}
                                {snapshot.llmConnectionsError && <span role="alert">{tr('llmConnectionsUnavailable', { error: snapshot.llmConnectionsError })}</span>}
                                {!snapshot.llmConnectionsLoading && snapshot.llmConnections?.length === 0 && <span>{tr('noLlmConnections')}</span>}
                                <button type="button" className="menu_button" disabled={snapshot.llmConnectionsLoading || snapshot.saving} onClick={() => void controller.refreshLlmConnections()}>{tr('refreshLlmConnections')}</button>
                            </div>
                        </div>
                    </>
                )}
                {warnings.length > 0 && (
                    <div className="ttas-binding-summary ttas-binding-warning ttas-span-2">
                        <i className="fa-solid fa-triangle-exclamation"></i>
                        <div>
                            <strong>{tr('agentProfileConfigurationWarnings')}</strong>
                            {warnings.map((warning) => (
                                <span key={warning}>{warning}</span>
                            ))}
                        </div>
                    </div>
                )}
            </div>
        </div>
    );
}
