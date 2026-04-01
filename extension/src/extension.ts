import * as vscode from 'vscode';
import * as path from 'path';
// eslint-disable-next-line @typescript-eslint/no-require-imports
const native = require('../cf_napi.node') as typeof import('../cf_napi.node');

import { ContextForgeCoreInstance } from './types';
import { registerContextProvider } from './providers/contextProvider';
import { registerSaveCommand } from './commands/save';
import { registerClearCommand } from './commands/clear';

export async function activate(context: vscode.ExtensionContext): Promise<void> {
    const outputChannel = vscode.window.createOutputChannel('Context Forge');
    context.subscriptions.push(outputChannel);
    outputChannel.appendLine(`[${new Date().toISOString()}] Context Forge activating...`);

    const dbPath = path.join(context.globalStorageUri.fsPath, 'context-forge.db');
    await vscode.workspace.fs.createDirectory(context.globalStorageUri);

    const core: ContextForgeCoreInstance = new native.ContextForgeCore(dbPath);
    context.subscriptions.push({ dispose: () => { void core.close(); } });

    const changeEmitter = new vscode.EventEmitter<void>();
    context.subscriptions.push(changeEmitter);

    registerContextProvider(context, core, outputChannel, changeEmitter);
    registerSaveCommand(context, core, outputChannel, changeEmitter);
    registerClearCommand(context, core, outputChannel, changeEmitter);

    outputChannel.appendLine(`[${new Date().toISOString()}] Context Forge activated successfully.`);
    outputChannel.appendLine(`[${new Date().toISOString()}] Database: ${dbPath}`);
    outputChannel.appendLine(`[${new Date().toISOString()}] Commands: context-forge.saveContext, context-forge.clearContext`);
}

export function deactivate(): void {
    // close() handled via context.subscriptions
}
