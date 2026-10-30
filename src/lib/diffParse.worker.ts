import { parseDiff } from "./diff";
import { flattenDiff } from "./diffView";

self.onmessage = (event: MessageEvent<string>) => {
  self.postMessage(flattenDiff(parseDiff(event.data).hunks));
};
