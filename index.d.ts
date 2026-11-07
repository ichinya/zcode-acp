export type QuotaWindow = { kind: "five-hour" | "daily" | "weekly" | "custom"; id: string; label: string; usedPercent: number; resetsAt: string | null; model: string | null; cost: { usedUsdCents: number; limitUsdCents: number } | null };
type QuotaFields = { plan: { id: string; multiplier: number | null } | null; accountEmail: string | null; planLabel: string | null };
export type QuotaUsage = QuotaFields & ({ status: "ok"; windows: QuotaWindow[] } | { status: "not_installed" | "unauthenticated" | "expired" } | { status: "error"; message: string });
export function collectPlanUsage(plan: "coding" | "start", options?: { env?: NodeJS.ProcessEnv; fetcher?: typeof fetch; now?: () => number; signal?: AbortSignal }): Promise<QuotaUsage>;
export function codingQuotaUsage(data: unknown, planLabel?: string): QuotaUsage;
export function startQuotaUsage(data: unknown, now?: number, serverDate?: number): QuotaUsage;
export function getAccountFingerprint(options?: { env?: NodeJS.ProcessEnv }): Promise<string>;
export function findNativeEntry(options?: { env?: NodeJS.ProcessEnv; platform?: string; home?: string }): string;
export function getRuntimeInfo(): { version: string; cliPath: string; revision: string };
export function nativeDoctor(cwd?: string): Promise<Record<string, unknown>>;
export class NativeAcpAgent {
  constructor(options?: Record<string, unknown>);
  handle(method: string, params?: unknown): Promise<unknown>;
  close(): Promise<void>;
}
