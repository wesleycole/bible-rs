/**
 * Finishing a sermon's transcription, shared by AssemblyAI's webhook and by GET /v1/sermons/:id
 * (which checks on a job whose webhook is late or never came, as on a developer's machine).
 * Either way the transcript is kept in Convex for the client, the audio's real duration is
 * settled, and nothing is left in Blob or AssemblyAI.
 */
import { Effect } from "effect"
import type { Id } from "../convex/_generated/dataModel.js"
import { Blob } from "./services/Blob.js"
import { Store } from "./services/Store.js"
import { type Job, segmentsOf, Transcriber } from "./services/Transcriber.js"

export interface SermonRef {
  readonly sermonId: string
  readonly status: string
  readonly blobPath: string
  readonly assemblyId?: string | undefined
}

/** Applies a finished (or failed) job to its sermon. Jobs still running are left alone. */
export const finishSermon = Effect.fn("sermon.finish")(function*(sermon: SermonRef, job: Job) {
  const store = yield* Store
  const blob = yield* Blob
  const transcriber = yield* Transcriber
  const sermonId = sermon.sermonId as Id<"sermons">
  if (job.status === "completed") {
    const transcript = {
      language: job.language_code ?? null,
      durationMs: Math.round((job.audio_duration ?? 0) * 1000),
      segments: segmentsOf(job)
    }
    yield* store.completeSermon({
      sermonId,
      transcript: JSON.stringify(transcript),
      audioSeconds: job.audio_duration ?? 0
    })
  } else if (job.status === "error") {
    yield* store.failSermon({ sermonId, error: job.error ?? "AssemblyAI couldn't transcribe it." })
  } else {
    return false
  }
  yield* transcriber.delete(job.id)
  yield* blob.delete([sermon.blobPath])
  return true
})

/** Checks on a sermon still transcribing, and finishes it if its job is done. */
export const reconcileSermon = Effect.fn("sermon.reconcile")(function*(sermon: SermonRef) {
  if (sermon.status !== "transcribing" || !sermon.assemblyId) return false
  const transcriber = yield* Transcriber
  const job = yield* transcriber.get(sermon.assemblyId)
  return yield* finishSermon(sermon, job)
})
