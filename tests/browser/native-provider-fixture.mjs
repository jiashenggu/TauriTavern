import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { randomUUID } from 'node:crypto';

// Only a synthetic localhost provider. No external model, API key or user chat.
export async function runNativeProviderSmoke(invoke) {
    let disconnect;
    const disconnected = new Promise((resolve) => { disconnect = resolve; });
    const server = createServer(async (request, response) => {
        if (request.url === '/v1/models') {
            response.setHeader('Content-Type', 'application/json');
            response.end(JSON.stringify({ object: 'list', data: [{ id: 'synthetic-model', object: 'model' }] }));
            return;
        }
        assert.equal(request.url, '/v1/chat/completions');
        let body = '';
        for await (const chunk of request) body += chunk;
        const payload = JSON.parse(body);
        response.setHeader('Content-Type', 'text/event-stream');
        response.write(`data: ${JSON.stringify({ choices: [{ index: 0, delta: { content: 'Synthetic token' } }] })}\n\n`);
        if (payload.messages[0].content === 'synthetic cancellation test') {
            const timer = setInterval(() => response.write(': heartbeat\n\n'), 250);
            response.on('close', () => {
                clearInterval(timer);
                disconnect();
            });
        } else {
            response.end('data: [DONE]\n\n');
        }
    });
    await new Promise((resolve, reject) => {
        server.once('error', reject);
        server.listen(1433, '127.0.0.1', resolve);
    });
    const dto = {
        chat_completion_source: 'custom', custom_api_format: 'openai_compat',
        custom_url: 'http://127.0.0.1:1433/v1',
    };
    try {
        const status = await invoke('get_chat_completions_status', { dto, locale: 'en' });
        assert.ok(JSON.stringify(status).includes('synthetic-model'), 'native provider status');
        for (const cancel of [false, true]) {
            const streamId = randomUUID();
            await invoke('start_chat_completion_stream', {
                streamId, locale: 'en', dto: {
                    ...dto, model: 'synthetic-model', stream: true, max_tokens: 32,
                    messages: [{ role: 'user', content: cancel ? 'synthetic cancellation test' : 'synthetic completion test' }],
                },
            });
            let afterSeq = 0;
            let sawChunk = false;
            let completed = false;
            const deadline = Date.now() + 15000;
            while (Date.now() < deadline) {
                const result = await invoke('read_chat_completion_stream', { streamId, afterSeq });
                for (const event of result.events) {
                    afterSeq = event.seq;
                    assert.notEqual(event.type, 'error', JSON.stringify(event));
                    if (event.type === 'chunk' && event.data.includes('Synthetic token')) sawChunk = true;
                }
                if (cancel && sawChunk) {
                    // Streaming cancellation is session close in the existing
                    // ai-routes contract; cancel_generation is for non-stream calls.
                    await invoke('close_chat_completion_stream', { streamId });
                    await Promise.race([
                        disconnected,
                        new Promise((_, reject) => setTimeout(() => reject(new Error('Native cancel did not close the provider stream')), 5000)),
                    ]);
                    completed = true;
                    break;
                }
                if (!cancel && result.status === 'done') {
                    completed = true;
                    break;
                }
                await new Promise((resolve) => setTimeout(resolve, 20));
            }
            assert.equal(sawChunk, true, 'native streaming token delivered');
            assert.equal(completed, true, 'native stream reached the expected terminal state');
            await invoke('close_chat_completion_stream', { streamId });
        }
        console.log('PASS: native model status, streaming token delivery and cancellation against a synthetic localhost provider');
    } finally {
        server.closeAllConnections();
        await new Promise((resolve) => server.close(resolve));
    }
}
