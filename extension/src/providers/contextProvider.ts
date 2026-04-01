import * as vscode from 'vscode';
import { ContextForgeCoreInstance } from '../types';

export function registerContextProvider(
    context: vscode.ExtensionContext,
    core: ContextForgeCoreInstance,
    outputChannel: vscode.OutputChannel,
    changeEmitter: vscode.EventEmitter<void>
): void {
    const provider: vscode.ChatWorkspaceContextProvider = {
        onDidChangeWorkspaceChatContext: changeEmitter.event,

        async provideWorkspaceChatContext(
            token: vscode.CancellationToken
        ): Promise<vscode.ChatContextItem[]> {
            if (token.isCancellationRequested) {
                return [];
            }

            try {
                const entries = await core.assemble('*');

                if (token.isCancellationRequested) {
                    return [];
                }

                if (entries.length === 0) {
                    outputChannel.appendLine(
                        `[${new Date().toISOString()}] ContextProvider: no entries to inject`
                    );
                    return [];
                }

                outputChannel.appendLine(
                    `[${new Date().toISOString()}] ContextProvider: injecting ${entries.length} entries into chat context`
                );

                return entries.map((entry) => ({
                    icon: new vscode.ThemeIcon('database'),
                    label: `Context Forge: ${entry.kind} (${new Date(entry.timestamp).toLocaleTimeString()})`,
                    modelDescription: `Preserved context from Context Forge (kind: ${entry.kind}, id: ${entry.id}, saved at ${new Date(entry.timestamp).toISOString()}). This context was saved before compaction and should be treated as important prior knowledge.`,
                    value: entry.content,
                }));
            } catch (err: unknown) {
                const message = err instanceof Error ? err.message : String(err);
                outputChannel.appendLine(
                    `[${new Date().toISOString()}] ContextProvider: error assembling context: ${message}`
                );
                return [];
            }
        },
    };

    const disposable = vscode.chat.registerChatWorkspaceContextProvider(
        'context-forge',
        provider
    );
    context.subscriptions.push(disposable);

    outputChannel.appendLine(
        `[${new Date().toISOString()}] ContextProvider: registered ChatWorkspaceContextProvider`
    );
}
