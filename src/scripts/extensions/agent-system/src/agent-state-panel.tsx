import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { AgentStatePanel } from './AgentStatePanel';
import { requireAgentApi, requireSillyTavernContext } from './host-api';
import { translateAgentSystem as tr } from './i18n';
import { subscribeAgentRunState } from '../../../tauritavern/agent/agent-run-controller.js';

type ContextEvents = {
    eventTypes: Record<string, string>;
    eventSource: {
        on: (name: string, listener: () => void) => void;
        removeListener: (name: string, listener: () => void) => void;
    };
};

function subscribeContext(listener: () => void): () => void {
    const { eventSource, eventTypes } = requireSillyTavernContext() as ContextEvents;
    const events = ['CHAT_CHANGED', 'MESSAGE_SWIPED', 'MESSAGE_DELETED'].map(key => eventTypes[key]).filter((name): name is string => Boolean(name));
    for (const event of events) eventSource.on(event, listener);
    const stop = subscribeAgentRunState(() => listener());
    return () => {
        stop();
        for (const event of events) eventSource.removeListener(event, listener);
    };
}

export function mountAgentStatePanel(): void {
    if (document.getElementById('ttas_agent_state_mount')) return;
    const mount = document.createElement('div');
    mount.id = 'ttas_agent_state_mount';
    document.body.append(mount);
    createRoot(mount).render(<StrictMode><AgentStatePanel api={requireAgentApi().state} tr={tr} subscribeContext={subscribeContext} /></StrictMode>);
}
