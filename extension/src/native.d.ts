declare module '*.node' {
    export type EntryKind = 'manual' | 'pre_compact' | 'auto';

    export interface JsContextEntry {
        id: string;
        content: string;
        timestamp: number;
        kind: EntryKind;
        tokenCount?: number;
    }

    export interface JsScoredEntry {
        entry: JsContextEntry;
        score: number;
    }

    export interface JsConfig {
        maxEntries?: number;
        tokenBudget?: number;
        evictionPolicy?: 'lru' | 'least_relevant';
    }

    export class ContextForgeCore {
        constructor(dbPath: string, config?: JsConfig);
        save(content: string, kind?: string): Promise<string>;
        assemble(query: string, tokenBudget?: number): Promise<JsContextEntry[]>;
        search(query: string, limit?: number): Promise<JsScoredEntry[]>;
        count(): Promise<number>;
        clear(): Promise<number>;
        delete(id: string): Promise<boolean>;
        getAll(): Promise<JsContextEntry[]>;
        close(): Promise<void>;
    }
}
