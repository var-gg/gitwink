import { readFile } from "node:fs/promises";

const locales = ["en", "ko", "ja"];
const catalogUrl = (locale) =>
  new URL(`../src/locales/${locale}/messages.po`, import.meta.url);

function poValue(lines, field) {
  const start = lines.findIndex((line) => line.startsWith(`${field} `));
  if (start < 0) return null;
  let value = JSON.parse(lines[start].slice(field.length + 1));
  for (let index = start + 1; index < lines.length; index += 1) {
    if (!lines[index].startsWith('"')) break;
    value += JSON.parse(lines[index]);
  }
  return value;
}

async function readCatalog(locale) {
  const source = await readFile(catalogUrl(locale), "utf8");
  const entries = new Map();
  for (const block of source.split(/\r?\n\r?\n/)) {
    if (!block.includes("js-lingui-explicit-id")) continue;
    const lines = block.split(/\r?\n/);
    const id = poValue(lines, "msgid");
    const translation = poValue(lines, "msgstr");
    if (!id) continue;
    if (entries.has(id)) throw new Error(`${locale}: duplicate message id ${id}`);
    entries.set(id, translation ?? "");
  }
  return entries;
}

const catalogs = new Map(
  await Promise.all(
    locales.map(async (locale) => [locale, await readCatalog(locale)]),
  ),
);
const sourceIds = [...catalogs.get("en").keys()].sort();
const failures = [];

for (const locale of locales) {
  const catalog = catalogs.get(locale);
  const ids = [...catalog.keys()].sort();
  const missingIds = sourceIds.filter((id) => !catalog.has(id));
  const extraIds = ids.filter((id) => !catalogs.get("en").has(id));
  const untranslated = ids.filter((id) => catalog.get(id).trim().length === 0);
  if (missingIds.length) failures.push(`${locale}: missing ids: ${missingIds.join(", ")}`);
  if (extraIds.length) failures.push(`${locale}: extra ids: ${extraIds.join(", ")}`);
  if (untranslated.length) {
    failures.push(`${locale}: empty translations: ${untranslated.join(", ")}`);
  }
}

if (failures.length) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
} else {
  console.log(
    `i18n catalogs complete: ${sourceIds.length} messages × ${locales.length} locales`,
  );
}
