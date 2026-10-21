/* eslint-disable */
/**
 * Generated `api` utility.
 *
 * THIS CODE IS AUTOMATICALLY GENERATED.
 *
 * To regenerate, run `npx convex dev`.
 * @module
 */

import type * as account from "../account.js";
import type * as admin from "../admin.js";
import type * as crons from "../crons.js";
import type * as entitlements from "../entitlements.js";
import type * as http from "../http.js";
import type * as lib_accounts from "../lib/accounts.js";
import type * as lib_allowance from "../lib/allowance.js";
import type * as lib_anchors from "../lib/anchors.js";
import type * as lib_chunks from "../lib/chunks.js";
import type * as lib_conflict from "../lib/conflict.js";
import type * as lib_digest from "../lib/digest.js";
import type * as lib_effect from "../lib/effect.js";
import type * as lib_errors from "../lib/errors.js";
import type * as lib_files from "../lib/files.js";
import type * as lib_perVerse from "../lib/perVerse.js";
import type * as lib_pricing from "../lib/pricing.js";
import type * as lib_purge from "../lib/purge.js";
import type * as lib_references from "../lib/references.js";
import type * as lib_sequence from "../lib/sequence.js";
import type * as lib_server from "../lib/server.js";
import type * as lib_services from "../lib/services.js";
import type * as lib_time from "../lib/time.js";
import type * as lib_transcriber from "../lib/transcriber.js";
import type * as lib_user from "../lib/user.js";
import type * as lib_verses from "../lib/verses.js";
import type * as lib_zip from "../lib/zip.js";
import type * as library_audio from "../library/audio.js";
import type * as library_bookmarks from "../library/bookmarks.js";
import type * as library_chats from "../library/chats.js";
import type * as library_chunk from "../library/chunk.js";
import type * as library_digest from "../library/digest.js";
import type * as library_export from "../library/export.js";
import type * as library_files from "../library/files.js";
import type * as library_highlights from "../library/highlights.js";
import type * as library_items from "../library/items.js";
import type * as library_notes from "../library/notes.js";
import type * as library_purge from "../library/purge.js";
import type * as library_rank from "../library/rank.js";
import type * as library_search from "../library/search.js";
import type * as library_sermons from "../library/sermons.js";
import type * as library_sync from "../library/sync.js";
import type * as library_whoami from "../library/whoami.js";
import type * as maintenance from "../maintenance.js";
import type * as rateLimits from "../rateLimits.js";
import type * as sources from "../sources.js";
import type * as usage from "../usage.js";
import type * as users from "../users.js";
import type * as webhooks from "../webhooks.js";

import type {
  ApiFromModules,
  FilterApi,
  FunctionReference,
} from "convex/server";

declare const fullApi: ApiFromModules<{
  account: typeof account;
  admin: typeof admin;
  crons: typeof crons;
  entitlements: typeof entitlements;
  http: typeof http;
  "lib/accounts": typeof lib_accounts;
  "lib/allowance": typeof lib_allowance;
  "lib/anchors": typeof lib_anchors;
  "lib/chunks": typeof lib_chunks;
  "lib/conflict": typeof lib_conflict;
  "lib/digest": typeof lib_digest;
  "lib/effect": typeof lib_effect;
  "lib/errors": typeof lib_errors;
  "lib/files": typeof lib_files;
  "lib/perVerse": typeof lib_perVerse;
  "lib/pricing": typeof lib_pricing;
  "lib/purge": typeof lib_purge;
  "lib/references": typeof lib_references;
  "lib/sequence": typeof lib_sequence;
  "lib/server": typeof lib_server;
  "lib/services": typeof lib_services;
  "lib/time": typeof lib_time;
  "lib/transcriber": typeof lib_transcriber;
  "lib/user": typeof lib_user;
  "lib/verses": typeof lib_verses;
  "lib/zip": typeof lib_zip;
  "library/audio": typeof library_audio;
  "library/bookmarks": typeof library_bookmarks;
  "library/chats": typeof library_chats;
  "library/chunk": typeof library_chunk;
  "library/digest": typeof library_digest;
  "library/export": typeof library_export;
  "library/files": typeof library_files;
  "library/highlights": typeof library_highlights;
  "library/items": typeof library_items;
  "library/notes": typeof library_notes;
  "library/purge": typeof library_purge;
  "library/rank": typeof library_rank;
  "library/search": typeof library_search;
  "library/sermons": typeof library_sermons;
  "library/sync": typeof library_sync;
  "library/whoami": typeof library_whoami;
  maintenance: typeof maintenance;
  rateLimits: typeof rateLimits;
  sources: typeof sources;
  usage: typeof usage;
  users: typeof users;
  webhooks: typeof webhooks;
}>;

/**
 * A utility for referencing Convex functions in your app's public API.
 *
 * Usage:
 * ```js
 * const myFunctionReference = api.myModule.myFunction;
 * ```
 */
export declare const api: FilterApi<
  typeof fullApi,
  FunctionReference<any, "public">
>;

/**
 * A utility for referencing Convex functions in your app's internal API.
 *
 * Usage:
 * ```js
 * const myFunctionReference = internal.myModule.myFunction;
 * ```
 */
export declare const internal: FilterApi<
  typeof fullApi,
  FunctionReference<any, "internal">
>;

export declare const components: {
  rateLimiter: import("@convex-dev/rate-limiter/_generated/component.js").ComponentApi<"rateLimiter">;
  workflow: import("@convex-dev/workflow/_generated/component.js").ComponentApi<"workflow">;
};
