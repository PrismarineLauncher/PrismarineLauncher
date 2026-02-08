// SPDX-FileCopyrightText: 2026
//
// SPDX-License-Identifier: GPL-3.0-only

#include "CustomContentAPI.h"

#include <QCryptographicHash>
#include <QDateTime>
#include <QDir>
#include <QFileInfo>
#include <QHash>
#include <QRegularExpression>
#include <QUrl>

#include <algorithm>

#include "Application.h"
#include "FileSystem.h"
#include "minecraft/mod/Mod.h"
#include "minecraft/mod/tasks/LocalModParseTask.h"
#include "modplatform/ModIndex.h"

namespace {

struct LocalPackEntry {
    ModPlatform::IndexedPack::Ptr pack;
    QDateTime modified;
};

struct GroupEntry {
    LocalPackEntry entry;
    QStringList fileNames;
};

QString sanitizeFilePart(const QString& value)
{
    QString out;
    out.reserve(value.size());
    for (QChar ch : value) {
        if (ch.isLetterOrNumber() || ch == '-' || ch == '_' || ch == '.')
            out += ch;
        else
            out += '-';
    }
    out.replace(QRegularExpression("-+"), "-");
    out.remove(QRegularExpression("(^-+|-+$)"));
    return out;
}

QString deriveFileName(const QString& slug, const QString& version, const QString& url)
{
    QUrl parsed(url);
    if (parsed.isValid()) {
        auto file = QFileInfo(parsed.path()).fileName();
        if (!file.isEmpty() && file.contains('.'))
            return file;
    }

    auto cleanSlug = sanitizeFilePart(slug);
    if (cleanSlug.isEmpty())
        cleanSlug = "mod";

    auto cleanVersion = sanitizeFilePart(version);
    if (cleanVersion.isEmpty())
        cleanVersion = "latest";

    return QString("%1-%2.jar").arg(cleanSlug, cleanVersion);
}

void appendLocalEntries(QList<LocalPackEntry>& entries, const ResourceAPI::SearchArgs& args)
{
    QDir root(APPLICATION->dataRoot());
    QDir custom_dir(root.filePath("CustomContent"));
    FS::ensureFolderPathExists(custom_dir.absolutePath());

    QStringList name_filters;
    name_filters << "*.jar" << "*.JAR";
    auto files = custom_dir.entryInfoList(name_filters, QDir::Files | QDir::NoDotAndDotDot);

    QString term;
    if (args.search.has_value())
        term = args.search.value();
    const bool byProjectId = term.startsWith('#');
    const QString projectId = byProjectId ? term.mid(1).trimmed() : QString();

    QHash<QString, GroupEntry> groups;

    for (auto const& file_info : files) {
        Mod mod(file_info);
        ModUtils::process(mod, ModUtils::ProcessingLevel::BasicInfoOnly);

        QString name = mod.name();
        if (name.isEmpty())
            name = file_info.completeBaseName();

        auto authors = mod.authors();
        QString key = name.toLower() + "|" + authors.join(",").toLower();

        auto& group = groups[key];
        if (!group.entry.pack) {
            auto pack = std::make_shared<ModPlatform::IndexedPack>();
            pack->provider = ModPlatform::ResourceProvider::CUSTOM;
            pack->addonId = QString::fromUtf8(QCryptographicHash::hash(key.toUtf8(), QCryptographicHash::Sha1).toHex());
            pack->slug = name;
            pack->logoName = file_info.fileName();
            pack->logoUrl = file_info.absoluteFilePath();

            pack->name = name;
            pack->description = mod.description();
            if (pack->description.isEmpty())
                pack->description = QObject::tr("Local mod file");

            for (auto const& author : authors) {
                ModPlatform::ModpackAuthor a;
                a.name = author;
                pack->authors.append(a);
            }

            pack->side = ModPlatform::SideUtils::fromString(mod.side());
            pack->versionsLoaded = true;
            pack->extraDataLoaded = true;

            group.entry.pack = pack;
            group.entry.modified = file_info.lastModified();
        } else {
            if (file_info.lastModified() > group.entry.modified)
                group.entry.modified = file_info.lastModified();
        }

        ModPlatform::IndexedVersion version;
        version.addonId = group.entry.pack->addonId;
        version.fileId = file_info.fileName();
        version.fileName = file_info.fileName();
        version.downloadUrl = QUrl::fromLocalFile(file_info.absoluteFilePath()).toString();
        version.date = file_info.lastModified().toString(Qt::ISODate);
        version.version_type = ModPlatform::IndexedVersionType::fromString(mod.releaseType());
        version.side = group.entry.pack->side;
        if (auto mc_versions = mod.mcVersions(); !mc_versions.isEmpty()) {
            auto parts = mc_versions.split(",", Qt::SkipEmptyParts);
            for (auto& part : parts)
                part = part.trimmed();
            version.mcVersion = parts;
        }

        QString version_str = mod.version();
        if (version_str.isEmpty())
            version_str = file_info.lastModified().toString("yyyy-MM-dd");
        version.version = QString("%1 [%2]").arg(version_str, file_info.fileName());
        version.version_number = version.version;

        group.entry.pack->versions.append(version);
        group.fileNames.append(file_info.fileName());
    }

    for (auto it = groups.begin(); it != groups.end(); ++it) {
        auto pack = it.value().entry.pack;

        if (byProjectId && !projectId.isEmpty()) {
            if (pack->addonId.toString() != projectId)
                continue;
        } else if (!term.isEmpty()) {
            bool matches = pack->name.contains(term, Qt::CaseInsensitive) || pack->description.contains(term, Qt::CaseInsensitive);
            if (!matches) {
                for (auto const& file_name : it.value().fileNames) {
                    if (file_name.contains(term, Qt::CaseInsensitive)) {
                        matches = true;
                        break;
                    }
                }
            }
            if (!matches)
                continue;
        }

        entries.append(it.value().entry);
    }
}

void appendTabEntries(QList<LocalPackEntry>& entries, const ResourceAPI::SearchArgs& args, const CustomContentTabs::TabDefinition& tab)
{
    QString term;
    if (args.search.has_value())
        term = args.search.value();
    const bool byProjectId = term.startsWith('#');
    const QString projectId = byProjectId ? term.mid(1).trimmed() : QString();

    for (auto const& src : tab.entries) {
        QFileInfo tabFileInfo(tab.sourcePath);
        QDir modtabsDir = tabFileInfo.absoluteDir();

        auto pack = std::make_shared<ModPlatform::IndexedPack>();
        pack->provider = ModPlatform::ResourceProvider::CUSTOM;
        pack->addonId = QString("customtab:%1:%2").arg(tab.id, src.slug);
        pack->slug = src.slug;
        pack->name = src.name.isEmpty() ? src.slug : src.name;
        pack->description = src.description.isEmpty() ? QObject::tr("Custom content entry") : src.description;
        if (!src.iconPath.isEmpty()) {
            pack->logoUrl = modtabsDir.absoluteFilePath(src.iconPath);
            pack->logoName = QFileInfo(pack->logoUrl).fileName();
        }
        pack->side = ModPlatform::Side::UniversalSide;
        pack->versionsLoaded = true;
        pack->extraDataLoaded = true;

        const bool hasEntryReadme = !src.readmeContent.isEmpty();
        const QString effectiveReadmeType = hasEntryReadme ? src.readmeType : tab.readmeType;
        const QString effectiveReadme = hasEntryReadme ? src.readmeContent : tab.readmeContent;
        if (effectiveReadmeType.compare("markdown", Qt::CaseInsensitive) == 0)
            pack->extraData.body = effectiveReadme;
        else if (!effectiveReadme.isEmpty())
            pack->description += "\n\n" + effectiveReadme;

        for (auto const& srcVersion : src.versions) {
            ModPlatform::IndexedVersion version;
            version.addonId = pack->addonId;
            version.fileId = srcVersion.id;
            version.version = srcVersion.id.compare("latest", Qt::CaseInsensitive) == 0 ? QObject::tr("Latest") : srcVersion.id;
            version.version_number = version.version;
            version.downloadUrl = srcVersion.url;
            version.fileName = srcVersion.fileName.isEmpty() ? deriveFileName(src.slug, srcVersion.id, srcVersion.url) : srcVersion.fileName;
            version.hash_type = srcVersion.hashType;
            version.hash = srcVersion.hash;
            version.side = ModPlatform::Side::UniversalSide;
            version.date = tab.sourceLastModified.toString(Qt::ISODate);
            if (srcVersion.id.compare("latest", Qt::CaseInsensitive) != 0)
                version.mcVersion = { srcVersion.id };

            pack->versions.append(version);
        }

        if (pack->versions.isEmpty())
            continue;

        if (byProjectId && !projectId.isEmpty()) {
            if (pack->addonId.toString() != projectId)
                continue;
        } else if (!term.isEmpty()) {
            bool matches = pack->name.contains(term, Qt::CaseInsensitive) || pack->slug.contains(term, Qt::CaseInsensitive) ||
                           pack->description.contains(term, Qt::CaseInsensitive);
            if (!matches) {
                for (auto const& version : pack->versions) {
                    if (version.version.contains(term, Qt::CaseInsensitive) || version.fileName.contains(term, Qt::CaseInsensitive)) {
                        matches = true;
                        break;
                    }
                }
            }
            if (!matches)
                continue;
        }

        entries.append({ pack, tab.sourceLastModified });
    }
}

void sortEntries(QList<LocalPackEntry>& entries, const ResourceAPI::SearchArgs& args)
{
    auto sort = args.sorting;
    if (sort.has_value() && (sort->name == "date" || sort->index == 1)) {
        std::sort(entries.begin(), entries.end(), [](const LocalPackEntry& a, const LocalPackEntry& b) {
            return a.modified > b.modified;
        });
    } else {
        std::sort(entries.begin(), entries.end(), [](const LocalPackEntry& a, const LocalPackEntry& b) {
            return QString::compare(a.pack->name, b.pack->name, Qt::CaseInsensitive) < 0;
        });
    }
}

class CustomContentSearchTask final : public Task {
   public:
    CustomContentSearchTask(ResourceAPI::SearchArgs args,
                            ResourceAPI::Callback<QList<ModPlatform::IndexedPack::Ptr>> callbacks,
                            std::optional<CustomContentTabs::TabDefinition> tab)
        : m_args(std::move(args)), m_callbacks(std::move(callbacks)), m_tab(std::move(tab))
    {}

   protected:
    void executeTask() override
    {
        QList<LocalPackEntry> entries;

        if (m_tab.has_value())
            appendTabEntries(entries, m_args, *m_tab);
        else
            appendLocalEntries(entries, m_args);

        sortEntries(entries, m_args);

        QList<ModPlatform::IndexedPack::Ptr> result;
        int offset = m_args.offset;
        int limit = 25;
        for (int i = offset; i < entries.size() && result.size() < limit; ++i)
            result.append(entries.at(i).pack);

        if (m_callbacks.on_succeed)
            m_callbacks.on_succeed(result);

        emitSucceeded();
    }

   private:
    ResourceAPI::SearchArgs m_args;
    ResourceAPI::Callback<QList<ModPlatform::IndexedPack::Ptr>> m_callbacks;
    std::optional<CustomContentTabs::TabDefinition> m_tab;
};

}  // namespace

auto CustomContentAPI::getSortingMethods() const -> QList<SortingMethod>
{
    return { { 0, "name", QObject::tr("Name") }, { 1, "date", QObject::tr("Date") } };
}

Task::Ptr CustomContentAPI::searchProjects(SearchArgs&& args, Callback<QList<ModPlatform::IndexedPack::Ptr>>&& callbacks) const
{
    return makeShared<CustomContentSearchTask>(std::move(args), std::move(callbacks), m_tab);
}

Task::Ptr CustomContentAPI::getProjects(QStringList, QByteArray*) const
{
    return nullptr;
}

auto CustomContentAPI::getSearchURL(SearchArgs const&) const -> std::optional<QString>
{
    return std::nullopt;
}

auto CustomContentAPI::getInfoURL(QString const&) const -> std::optional<QString>
{
    return std::nullopt;
}

auto CustomContentAPI::getVersionsURL(VersionSearchArgs const&) const -> std::optional<QString>
{
    return std::nullopt;
}

auto CustomContentAPI::getDependencyURL(DependencySearchArgs const&) const -> std::optional<QString>
{
    return std::nullopt;
}

void CustomContentAPI::loadIndexedPack(ModPlatform::IndexedPack&, QJsonObject&) const {}

ModPlatform::IndexedVersion CustomContentAPI::loadIndexedPackVersion(QJsonObject&, ModPlatform::ResourceType) const
{
    return {};
}

QJsonArray CustomContentAPI::documentToArray(QJsonDocument&) const
{
    return {};
}

void CustomContentAPI::loadExtraPackInfo(ModPlatform::IndexedPack&, QJsonObject&) const {}
