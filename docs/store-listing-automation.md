# Microsoft Store listing asset automation

gitwink submits Store packages and approved listing screenshots through the
packaged-app submission API. This is a browser-free release path: GitHub Actions
uploads the exact MSIX bundle and PNG files checked out from the release commit.
Partner Center remains the place to inspect certification and final publication
state, not a required data-entry surface for every release.

## Screenshot manifest

store/listing/screenshots.json is the source of truth. Screenshot paths are
relative to store/listing/assets, and their array order is their Store order:

    {
      "$schema": "./screenshots.schema.json",
      "productId": "9P0S21GJD53F",
      "locales": {
        "en-us": [
          "en-us/01-timeline.png",
          "en-us/02-file-history.png"
        ]
      }
    }

The replacement boundary is deliberately narrow:

- A locale omitted from locales keeps all of its existing screenshots.
- A locale present in locales replaces its complete desktop screenshot set in
  the listed order.
- Store logos, promotional art, non-desktop device images, and every unlisted
  locale are preserved.
- The localized listing must already exist. The tool will not invent or copy
  titles and descriptions while uploading images.
- An empty locales object is valid and is the normal choice when a release does
  not need new screenshots.

Microsoft Store update submissions inherit the previous listing, so screenshots
do not need to be uploaded for every package version. Add a locale only when the
reviewed image set should actually change.

## Validation

Run:

    pnpm store:listing:check
    pwsh tools/publish-store.ps1

Both commands are read-only. The validator rejects:

- paths outside store/listing/assets, backslash paths, and duplicates;
- non-PNG files or malformed PNG headers;
- files larger than 50 MB;
- desktop images below 1366x768 landscape or 768x1366 portrait;
- empty locale arrays and more than 10 desktop screenshots.

One screenshot is accepted, but the tool warns below Microsoft's recommendation
of four. File content is SHA-256-addressed in the upload ZIP, so a changed PNG
cannot collide with a prior Store filename.

## Submission transaction

tools/publish-store.ps1 -PackagePath PATH -Publish is the only mutating entry
point. GitHub Actions normally owns that invocation. It:

1. validates local files before authentication;
2. refuses to run if any pending submission already exists;
3. preflights package and locale ownership against the last published
   submission;
4. creates one API-owned draft;
5. marks the previous package and only the selected locale screenshots for
   deletion;
6. uploads one ZIP containing the new package and pending PNGs;
7. commits once and polls until Partner Center accepts or rejects ingestion.

Before commit, a failure rolls back only the draft created by that invocation.
After a commit request, the draft is preserved because deleting an
indeterminate server-side operation would be unsafe. Logs include the submission
ID and available correlation ID, but never the access token, client secret, or
SAS upload URL.

Transient read failures are retried without rebuilding the submission. GET
requests retry transport failures plus HTTP 408, 429, and 5xx responses up to
four total attempts, honoring Retry-After before bounded exponential backoff.
POST, PUT, and DELETE remain single-attempt operations: automatically repeating
a mutation after a lost response could create a second draft or hide an
indeterminate commit.

Every submission API request declares the application/json media type, including
bodyless create and commit POSTs. Those endpoints still receive no synthetic
request body; the explicit media type satisfies the Store API contract without
changing the documented mutation payload.

Do not edit an API-created draft in Partner Center while the workflow is
running. Conversely, the workflow never modifies or deletes a draft that
already existed when it started. The workflow concurrency key also prevents two
gitwink Store submissions from racing.

## CI credentials and result meaning

The repository variable STORE_AUTOPUBLISH=true enables submission when these
secrets are present:

- AZURE_AD_TENANT_ID
- AZURE_AD_APPLICATION_CLIENT_ID
- AZURE_AD_APPLICATION_SECRET

The client credential is sent only to Microsoft Entra for the
https://manage.devcenter.microsoft.com/.default scope. The submission API
receives the resulting bearer token and tenant ID.

A green workflow means Partner Center accepted the commit for downstream
processing. It does not claim certification or public catalog propagation is
finished; those remain asynchronous Store states. The gitwink-msix Actions
artifact is uploaded before submission and remains the exact manual package
fallback if the API path fails.

## Official references

- [Manage app submissions](https://learn.microsoft.com/en-us/windows/uwp/monetize/manage-app-submissions)
- [Create an app submission](https://learn.microsoft.com/en-us/windows/uwp/monetize/create-an-app-submission)
- [Microsoft Store submission API](https://learn.microsoft.com/en-us/windows/apps/publish/store-submission-api)
- [MSIX screenshot requirements](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msix/screenshots-and-images)
