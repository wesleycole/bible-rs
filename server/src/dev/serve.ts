/** The proxy on a local Node server (port 3000 or PORT), with the same routes Vercel serves. */
import { NodeHttpServer, NodeRuntime } from "@effect/platform-node"
import { Layer } from "effect"
import { HttpRouter } from "effect/http"
import { createServer } from "node:http"
import { Routes, Services } from "../Main.js"

const port = Number(process.env.PORT ?? 3000)

HttpRouter.serve(Routes.pipe(Layer.provide(Services))).pipe(
  Layer.provide(NodeHttpServer.layer(createServer, { port, host: "127.0.0.1" })),
  Layer.launch,
  NodeRuntime.runMain
)
