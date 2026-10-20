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
import type * as lib_accounts from "../lib/accounts.js";
import type * as lib_anchors from "../lib/anchors.js";
import type * as lib_conflict from "../lib/conflict.js";
import type * as lib_effect from "../lib/effect.js";
import type * as lib_errors from "../lib/errors.js";
import type * as lib_files from "../lib/files.js";
import type * as lib_perVerse from "../lib/perVerse.js";
import type * as lib_pricing from "../lib/pricing.js";
import type * as lib_sequence from "../lib/sequence.js";
import type * as lib_server from "../lib/server.js";
import type * as lib_services from "../lib/services.js";
import type * as lib_time from "../lib/time.js";
import type * as lib_user from "../lib/user.js";
import type * as lib_verses from "../lib/verses.js";
import type * as library_bookmarks from "../library/bookmarks.js";
import type * as library_chats from "../library/chats.js";
import type * as library_files from "../library/files.js";
import type * as library_highlights from "../library/highlights.js";
import type * as library_notes from "../library/notes.js";
import type * as library_sync from "../library/sync.js";
import type * as library_whoami from "../library/whoami.js";
import type * as maintenance from "../maintenance.js";
import type * as rateLimits from "../rateLimits.js";
import type * as sermons from "../sermons.js";
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
  "lib/accounts": typeof lib_accounts;
  "lib/anchors": typeof lib_anchors;
  "lib/conflict": typeof lib_conflict;
  "lib/effect": typeof lib_effect;
  "lib/errors": typeof lib_errors;
  "lib/files": typeof lib_files;
  "lib/perVerse": typeof lib_perVerse;
  "lib/pricing": typeof lib_pricing;
  "lib/sequence": typeof lib_sequence;
  "lib/server": typeof lib_server;
  "lib/services": typeof lib_services;
  "lib/time": typeof lib_time;
  "lib/user": typeof lib_user;
  "lib/verses": typeof lib_verses;
  "library/bookmarks": typeof library_bookmarks;
  "library/chats": typeof library_chats;
  "library/files": typeof library_files;
  "library/highlights": typeof library_highlights;
  "library/notes": typeof library_notes;
  "library/sync": typeof library_sync;
  "library/whoami": typeof library_whoami;
  maintenance: typeof maintenance;
  rateLimits: typeof rateLimits;
  sermons: typeof sermons;
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
};
