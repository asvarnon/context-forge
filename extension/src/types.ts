import type NativeMod from '../cf_napi.node';

export type ContextForgeCoreInstance = InstanceType<typeof NativeMod.ContextForgeCore>;
export type NativeContextEntry = NativeMod.JsContextEntry;
export type NativeScoredEntry = NativeMod.JsScoredEntry;
