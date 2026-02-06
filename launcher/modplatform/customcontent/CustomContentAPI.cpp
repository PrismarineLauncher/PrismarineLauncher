// SPDX-FileCopyrightText: 2026
//
// SPDX-License-Identifier: GPL-3.0-only

#include "CustomContentAPI.h"

#include <QDateTime>
#include <QDir>
#include <QFileInfo>
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

class CustomContentSearchTask final : public Task {
   public:
    CustomContentSearchTask(ResourceAPI::SearchArgs args,
                            ResourceAPI::Callback<QList<ModPlatform::IndexedPack::Ptr>> callbacks)
        : m_args(std::move(args)), m_callbacks(std::move(callbacks))
    {}

   protected:
    void executeTask() override
    {
        QList<LocalPackEntry> entries;

        QDir root(APPLICATION->dataRoot());
        QDir custom_dir(root.filePath("CustomContent"));
        FS::ensureFolderPathExists(custom_dir.absolutePath());

        QStringList name_filters;
        name_filters << "*.jar" << "*.JAR";
        auto files = custom_dir.entryInfoList(name_filters, QDir::Files | QDir::NoDotAndDotDot);

        QString term;
        if (m_args.search.has_value())
            term = m_args.search.value();

        for (auto const& file_info : files) {
            Mod mod(file_info);
            ModUtils::process(mod, ModUtils::ProcessingLevel::BasicInfoOnly);

            auto pack = std::make_shared<ModPlatform::IndexedPack>();
            pack->provider = ModPlatform::ResourceProvider::CUSTOM;
            pack->addonId = file_info.fileName();
            pack->slug = file_info.completeBaseName();
            pack->logoName = file_info.fileName();
            pack->logoUrl = file_info.absoluteFilePath();

            QString name = mod.name();
            if (name.isEmpty())
                name = file_info.completeBaseName();
            pack->name = name;

            pack->description = mod.description();
            if (pack->description.isEmpty())
                pack->description = QObject::tr("Local mod file");

            for (auto const& author : mod.authors()) {
                ModPlatform::ModpackAuthor a;
                a.name = author;
                pack->authors.append(a);
            }

            pack->side = ModPlatform::SideUtils::fromString(mod.side());

            ModPlatform::IndexedVersion version;
            version.addonId = pack->addonId;
            version.fileId = file_info.fileName();
            version.fileName = file_info.fileName();
            version.downloadUrl = QUrl::fromLocalFile(file_info.absoluteFilePath()).toString();
            version.date = file_info.lastModified().toString(Qt::ISODate);
            version.version_type = ModPlatform::IndexedVersionType::fromString(mod.releaseType());
            version.side = pack->side;

            QString version_str = mod.version();
            if (version_str.isEmpty())
                version_str = file_info.lastModified().toString("yyyy-MM-dd");
            version.version = version_str;
            version.version_number = version_str;

            pack->versionsLoaded = true;
            pack->versions = { version };
            pack->extraDataLoaded = true;

            if (!term.isEmpty()) {
                auto matches = pack->name.contains(term, Qt::CaseInsensitive) ||
                               pack->description.contains(term, Qt::CaseInsensitive) ||
                               file_info.fileName().contains(term, Qt::CaseInsensitive);
                if (!matches)
                    continue;
            }

            entries.append({ pack, file_info.lastModified() });
        }

        auto sort = m_args.sorting;
        if (sort.has_value() && (sort->name == "date" || sort->index == 1)) {
            std::sort(entries.begin(), entries.end(), [](const LocalPackEntry& a, const LocalPackEntry& b) {
                return a.modified > b.modified;
            });
        } else {
            std::sort(entries.begin(), entries.end(), [](const LocalPackEntry& a, const LocalPackEntry& b) {
                return QString::compare(a.pack->name, b.pack->name, Qt::CaseInsensitive) < 0;
            });
        }

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
};

}  // namespace

auto CustomContentAPI::getSortingMethods() const -> QList<SortingMethod>
{
    return { { 0, "name", QObject::tr("Name") }, { 1, "date", QObject::tr("Date") } };
}

Task::Ptr CustomContentAPI::searchProjects(SearchArgs&& args, Callback<QList<ModPlatform::IndexedPack::Ptr>>&& callbacks) const
{
    return makeShared<CustomContentSearchTask>(std::move(args), std::move(callbacks));
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
