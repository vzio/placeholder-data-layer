import type { DatasetRequest } from "./generated/DatasetRequest";
import type { AnonymizeResponse } from "./generated/AnonymizeResponse";
import type { ProfileResponse } from "./generated/ProfileResponse";
import type { Problem } from "./generated/Problem";
import type { Health } from "./generated/Health";
import type { SemanticType } from "./generated/SemanticType";
import type { JobSummary } from "./generated/JobSummary";

export type { DatasetRequest } from "./generated/DatasetRequest";
export type { DatasetDto } from "./generated/DatasetDto";
export type { TableDto } from "./generated/TableDto";
export type { ColumnDto } from "./generated/ColumnDto";
export type { WireType } from "./generated/WireType";
export type { OptionsDto } from "./generated/OptionsDto";
/**
 * A single cell, as this client returns it. The integer arm types as
 * `number | string` rather than plain `number`: an `i64` outside
 * ±(2^53 − 1) cannot survive a JS `number` without losing precision, so a
 * value in that range arrives as the exact digit string instead of the
 * rounded double `JSON.parse` would otherwise have produced. See
 * {@link DataLayer} for how the response is parsed to make that true.
 */
export type { Value } from "./generated/Value";
export type { ForeignKey } from "./generated/ForeignKey";
export type { FidelityReport } from "./generated/FidelityReport";
export type { ColumnReport } from "./generated/ColumnReport";
export type { AnomalyRates } from "./generated/AnomalyRates";
export type { AnomalyKind } from "./generated/AnomalyKind";
export type { Check } from "./generated/Check";
export type { CheckOutcome } from "./generated/CheckOutcome";
export type { Verdict } from "./generated/Verdict";
export type { Strategy } from "./generated/Strategy";
export type { Strength } from "./generated/Strength";
export type { PreserveReason } from "./generated/PreserveReason";
export type { Stage } from "./generated/Stage";
export type { DatasetProfile } from "./generated/DatasetProfile";
export type { ColumnProfile } from "./generated/ColumnProfile";
export type { LengthStats } from "./generated/LengthStats";
export type { Distribution } from "./generated/Distribution";
export type { CategoricalProfile } from "./generated/CategoricalProfile";
export type { NumericProfile } from "./generated/NumericProfile";
export type { Bucket } from "./generated/Bucket";
export type { SemanticType } from "./generated/SemanticType";
export type { Problem } from "./generated/Problem";
export type { Health } from "./generated/Health";
export type { JobSummary } from "./generated/JobSummary";
export type { JobId } from "./generated/JobId";
export type { JobKind } from "./generated/JobKind";
export type { JobState } from "./generated/JobState";
export type { AnonymizeResponse } from "./generated/AnonymizeResponse";
export type { ProfileResponse } from "./generated/ProfileResponse";

const PROBLEM_CONTENT_TYPE = "application/problem+json";

/** How long {@link DataLayer.connect} waits for `/health` before giving up. */
const CONNECT_TIMEOUT_MS = 5000;

/**
 * The third argument a `JSON.parse` reviver receives on a runtime that
 * supports source-text access (Node 21+, every evergreen browser): the
 * exact slice of the input that produced this value, before any numeric
 * rounding. Where the runtime predates this, `source` is simply absent and
 * every value parses exactly as plain `JSON.parse` would.
 */
interface JsonParseContext {
  source?: string;
}

const BARE_INTEGER = /^-?\d+$/;

/**
 * `JSON.parse`, except an integer literal outside ±(2^53 − 1) survives as
 * the exact digit string it was written as, rather than silently rounding
 * to the nearest representable double. `Value::Integer` on the wire is an
 * `i64`, and a minted pseudonym preserves the input's magnitude, so an
 * 18-19 digit key column can produce distinct pseudonyms that plain
 * `JSON.parse` cannot tell apart. Every literal that already fits a safe
 * integer parses exactly as `JSON.parse` would; nothing about an ordinary
 * count or timestamp changes.
 */
function parseJsonPreservingIntegers(text: string): unknown {
  const reviver = (_key: string, value: unknown, context: JsonParseContext): unknown => {
    if (
      typeof value === "number" &&
      typeof context?.source === "string" &&
      BARE_INTEGER.test(context.source) &&
      !Number.isSafeInteger(value)
    ) {
      return context.source;
    }
    return value;
  };
  return JSON.parse(text, reviver as (key: string, value: unknown) => unknown);
}

async function readJson<T>(response: Response): Promise<T> {
  const text = await response.text();
  return parseJsonPreservingIntegers(text) as T;
}

/**
 * Raised by {@link DataLayer.connect} when the configured base URL does not
 * answer `/health`. The failure surfaces at construction, not on the first
 * `anonymize` call a service happens to make hours later.
 */
export class DataLayerConnectionError extends Error {
  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "DataLayerConnectionError";
  }
}

/**
 * Raised when a request fails in a way the server did not describe as an
 * RFC 9457 problem — a transport failure, or a response this client cannot
 * parse. A response the server *did* describe that way is not thrown: it is
 * returned as the `success: false` branch of the result, since the caller
 * already has everything needed to act on it.
 */
export class DataLayerRequestError extends Error {
  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "DataLayerRequestError";
  }
}

export type AnonymizeResult = ({ success: true } & AnonymizeResponse) | { success: false; error: Problem };

export type ProfileResult = ({ success: true } & ProfileResponse) | { success: false; error: Problem };

/**
 * HTTP client for the Data Layer anonymization service.
 *
 * Construct it with {@link DataLayer.connect}, never directly: a service
 * pointed at the wrong URL should fail there, before it has accepted any
 * traffic, rather than on the first dataset it tries to anonymize.
 */
export class DataLayer {
  private readonly baseUrl: string;

  private constructor(baseUrl: string) {
    this.baseUrl = baseUrl;
  }

  static async connect(baseUrl: string): Promise<DataLayer> {
    const normalized = baseUrl.replace(/\/+$/, "");
    const url = `${normalized}/health`;

    let response: Response;
    try {
      response = await fetch(url, { signal: AbortSignal.timeout(CONNECT_TIMEOUT_MS) });
    } catch (cause) {
      throw new DataLayerConnectionError(`could not reach ${url}`, { cause });
    }
    if (!response.ok) {
      throw new DataLayerConnectionError(`${url} responded ${response.status}`);
    }

    let health: Health;
    try {
      health = await readJson<Health>(response);
    } catch (cause) {
      throw new DataLayerConnectionError(`${url} did not return a JSON health body`, { cause });
    }
    if (health.status !== "ok") {
      // Any 2xx with a JSON body would otherwise connect successfully, which
      // is exactly the misconfiguration this factory exists to catch: a URL
      // pointing at an unrelated service that happens to answer `/health`.
      throw new DataLayerConnectionError(
        `${url} reported status ${JSON.stringify(health.status)}, not "ok"`,
      );
    }

    return new DataLayer(normalized);
  }

  /** The full pipeline: classify, profile, detect anomalies, transform, check. */
  async anonymize(request: DatasetRequest): Promise<AnonymizeResult> {
    const outcome = await this.post<AnonymizeResponse>("/v1/anonymize", request);
    return outcome.success ? { success: true, ...outcome.body } : outcome;
  }

  /** Steps 2-3 only: classify, profile, detect anomalies — no transformation. */
  async profile(request: DatasetRequest): Promise<ProfileResult> {
    const outcome = await this.post<ProfileResponse>("/v1/profile", request);
    return outcome.success ? { success: true, ...outcome.body } : outcome;
  }

  /** Semantic types this build recognizes. */
  async detectors(): Promise<SemanticType[]> {
    return this.get<SemanticType[]>("/v1/detectors");
  }

  /** In-flight jobs, for worker-pool observability. Never carries dataset contents. */
  async jobs(): Promise<JobSummary[]> {
    return this.get<JobSummary[]>("/v1/jobs");
  }

  private async post<T>(
    path: string,
    body: unknown,
  ): Promise<{ success: true; body: T } | { success: false; error: Problem }> {
    const url = `${this.baseUrl}${path}`;
    let response: Response;
    try {
      response = await fetch(url, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
    } catch (cause) {
      throw new DataLayerRequestError(`${url} did not respond`, { cause });
    }

    if (response.ok) {
      return { success: true, body: await readJson<T>(response) };
    }

    const problem = await readProblem(response);
    if (problem) {
      return { success: false, error: problem };
    }
    throw new DataLayerRequestError(`${url} responded ${response.status} without a problem body`);
  }

  private async get<T>(path: string): Promise<T> {
    const url = `${this.baseUrl}${path}`;
    let response: Response;
    try {
      response = await fetch(url);
    } catch (cause) {
      throw new DataLayerRequestError(`${url} did not respond`, { cause });
    }

    if (response.ok) {
      return readJson<T>(response);
    }

    const problem = await readProblem(response);
    throw new DataLayerRequestError(problem?.detail ?? `${url} responded ${response.status}`);
  }
}

async function readProblem(response: Response): Promise<Problem | null> {
  const contentType = response.headers.get("content-type") ?? "";
  if (!contentType.includes(PROBLEM_CONTENT_TYPE)) {
    return null;
  }
  try {
    return await readJson<Problem>(response);
  } catch {
    return null;
  }
}
