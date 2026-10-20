import { cronJobs } from "convex/server"
import { internal } from "./_generated/api.js"

const crons = cronJobs()

crons.interval("sweep reservations", { minutes: 5 }, internal.usage.sweepReservations)
crons.interval("fail stuck sermons", { hours: 1 }, internal.maintenance.failStuckSermons)
crons.interval("expire transcripts", { hours: 1 }, internal.sermons.expire)
crons.interval("check transcription jobs", { hours: 1 }, internal.library.sermons.checkJobs, {})
crons.interval("finish dead recordings", { hours: 1 }, internal.library.sermons.finishDead, {})
crons.daily("reconcile App Store", { hourUTC: 7, minuteUTC: 0 }, internal.maintenance.reconcileAppStore)
crons.daily("prune questions", { hourUTC: 7, minuteUTC: 15 }, internal.maintenance.pruneQuestions)
crons.daily("prune ledger", { hourUTC: 7, minuteUTC: 30 }, internal.maintenance.pruneLedger)
crons.daily("prune webhook events", { hourUTC: 7, minuteUTC: 45 }, internal.webhooks.prune)
crons.daily("purge library tombstones", { hourUTC: 8, minuteUTC: 0 }, internal.library.purge.tombstones)
crons.daily("delete orphaned blobs", { hourUTC: 8, minuteUTC: 15 }, internal.library.purge.orphans, {})

export default crons
