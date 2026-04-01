import * as vscode from 'vscode';
import { ContextForgeCoreInstance } from '../types';

export function registerSaveCommand(
    context: vscode.ExtensionContext,
    core: ContextForgeCoreInstance,
    outputChannel: vscode.OutputChannel,
    changeEmitter: vscode.EventEmitter<void>
): void {
    const disposable = vscode.commands.registerCommand(
        'context-forge.saveContext',
        async (args?: { content?: string; kind?: string }) => {
            const content = args?.content ?? `Context snapshot at ${new Date().toISOString()}`;
            const validKinds: ReadonlySet<string> = new Set(['manual', 'pre_compact', 'auto']);
            const kind = args?.kind && validKinds.has(args.kind) ? args.kind : 'manual';

            try {
                const id = await core.save(content, kind);
                changeEmitter.fire();
                outputChannel.appendLine(
                    `[${new Date().toISOString()}] SaveCommand: saved entry ${id} (kind: ${kind}, length: ${content.length})`
                );
            } catch (err: unknown) {
                const message = err instanceof Error ? err.message : String(err);
                outputChannel.appendLine(
                    `[${new Date().toISOString()}] SaveCommand: error saving context: ${message}`
                );
                void vscode.window.showErrorMessage(`Context Forge: failed to save — ${message}`);
            }
        }
    );
    context.subscriptions.push(disposable);
}
