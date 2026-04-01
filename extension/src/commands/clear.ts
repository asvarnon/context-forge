import * as vscode from 'vscode';
import { ContextForgeCoreInstance } from '../types';

export function registerClearCommand(
    context: vscode.ExtensionContext,
    core: ContextForgeCoreInstance,
    outputChannel: vscode.OutputChannel,
    changeEmitter: vscode.EventEmitter<void>
): void {
    const disposable = vscode.commands.registerCommand(
        'context-forge.clearContext',
        async () => {
            try {
                const count = await core.clear();
                changeEmitter.fire();
                outputChannel.appendLine(
                    `[${new Date().toISOString()}] ClearCommand: cleared ${count} entries`
                );
            } catch (err: unknown) {
                const message = err instanceof Error ? err.message : String(err);
                outputChannel.appendLine(
                    `[${new Date().toISOString()}] ClearCommand: error clearing context: ${message}`
                );
                void vscode.window.showErrorMessage(`Context Forge: failed to clear — ${message}`);
            }
        }
    );
    context.subscriptions.push(disposable);
}
