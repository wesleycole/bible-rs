/**
 * The one Vercel Function: every /v1/* request, served by the Effect app as a Web-standard handler.
 * Fluid compute lets one instance carry many concurrent Ask streams.
 */
import { handler } from "../src/Main.js"

export default {
  fetch(request: Request): Promise<Response> {
    // Routed here by vercel.json's rewrite of /v1/*; the app's routes start at /v1.
    const url = new URL(request.url)
    if (url.pathname.startsWith("/api/")) {
      url.pathname = url.pathname.slice("/api".length)
      return handler(new Request(url, request))
    }
    return handler(request)
  }
}
