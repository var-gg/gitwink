import type { I18n } from "@lingui/core";

import type { CommitMenuLabels } from "../lib/commitClipboard";
import { diffMessages } from "./messages/diff";

/** UI copy stays in the catalog; clipboard command assembly receives a
 * complete label set and remains deterministic and React-independent. */
export function commitMenuLabels(i18n: I18n): CommitMenuLabels {
  return {
    copy: i18n._(diffMessages.copy),
    copyWithReference: i18n._(diffMessages.copyWithReference),
    showFileHistory: i18n._(diffMessages.showFileHistoryDecorated),
    showFileHistoryAria: i18n._(diffMessages.showFileHistory),
    copyFilePath: i18n._(diffMessages.copyFilePath),
    copyAiContext: i18n._(diffMessages.copyAiContext),
    copyCommitMessage: i18n._(diffMessages.copyCommitMessage),
    copyShortHash: i18n._(diffMessages.copyShortHash),
    copyFullHash: i18n._(diffMessages.copyFullHash),
  };
}
