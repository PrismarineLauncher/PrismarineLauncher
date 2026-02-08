// SPDX-FileCopyrightText: 2026
//
// SPDX-License-Identifier: GPL-3.0-only

#include "CustomTabConfig.h"

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonParseError>
#include <QJsonValue>
#include <QLoggingCategory>
#include <QRegularExpression>
#include <QSet>

#include <toml++/toml.h>

Q_LOGGING_CATEGORY(logCustomTabs, "launcher.customcontent.tabs")

namespace CustomContentTabs {
namespace {

QString trimQuotes(QString value)
{
    value = value.trimmed();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith('\'') && value.endsWith('\''))) {
        if (value.size() >= 2)
            return value.mid(1, value.size() - 2);
    }
    return value;
}

QString sanitizeToken(const QString& token)
{
    QString out;
    out.reserve(token.size());
    for (QChar ch : token) {
        if (ch.isLetterOrNumber())
            out += ch.toLower();
        else
            out += '-';
    }
    out.replace(QRegularExpression("-+"), "-");
    out.remove(QRegularExpression("(^-+|-+$)"));
    if (out.isEmpty())
        out = "tab";
    return out;
}

QString makeTabId(const QFileInfo& info)
{
    return QString("customtab.%1").arg(sanitizeToken(info.completeBaseName()));
}

bool isSupportedSuffix(const QString& suffix)
{
    auto lower = suffix.toLower();
    return lower == "json" || lower == "toml" || lower == "yaml" || lower == "yml";
}

QString takeString(const QJsonValue& value)
{
    return value.isString() ? value.toString() : QString();
}

VersionEntry parseJsonVersion(const QString& versionId, const QJsonValue& value)
{
    VersionEntry version;
    version.id = versionId;

    if (value.isString()) {
        version.url = value.toString();
        return version;
    }

    const auto obj = value.toObject();
    version.url = obj.value("url").toString();
    version.fileName = obj.value("file_name").toString();
    version.hashType = obj.value("hash_type").toString();
    version.hash = obj.value("hash").toString();
    return version;
}

TabDefinition parseJson(const QFileInfo& fileInfo, const QByteArray& data)
{
    TabDefinition tab;
    tab.id = makeTabId(fileInfo);
    tab.sourcePath = fileInfo.absoluteFilePath();
    tab.sourceLastModified = fileInfo.lastModified();
    tab.readmeType = "markdown";

    QJsonParseError error{};
    auto document = QJsonDocument::fromJson(data, &error);
    if (error.error != QJsonParseError::NoError || !document.isObject()) {
        qWarning(logCustomTabs) << "Failed to parse custom tab json:" << fileInfo.fileName() << error.errorString();
        return {};
    }

    auto root = document.object();
    tab.name = takeString(root.value("name"));

    auto readmeObj = root.value("readme").toObject();
    if (!readmeObj.isEmpty()) {
        auto readmeType = takeString(readmeObj.value("type"));
        if (!readmeType.isEmpty())
            tab.readmeType = readmeType;
        tab.readmeContent = takeString(readmeObj.value("content"));
    }

    auto contentObj = root.value("content").toObject();
    for (auto it = contentObj.constBegin(); it != contentObj.constEnd(); ++it) {
        ContentEntry entry;
        entry.slug = it.key();
        entry.name = entry.slug;

        auto entryObj = it.value().toObject();
        if (entryObj.contains("name"))
            entry.name = takeString(entryObj.value("name"));
        entry.description = takeString(entryObj.value("description"));

        for (auto vit = entryObj.constBegin(); vit != entryObj.constEnd(); ++vit) {
            if (vit.key() == "name" || vit.key() == "description")
                continue;
            auto version = parseJsonVersion(vit.key(), vit.value());
            if (!version.url.isEmpty())
                entry.versions.append(version);
        }

        if (!entry.versions.isEmpty())
            tab.entries.append(entry);
    }

    if (tab.name.isEmpty())
        tab.name = fileInfo.completeBaseName();

    return tab;
}

QString tomlString(const toml::node* node)
{
    if (!node || !node->is_string())
        return {};
    return QString::fromStdString(node->as_string()->value_or(""));
}

VersionEntry parseTomlVersion(const QString& versionId, const toml::node* node)
{
    VersionEntry version;
    version.id = versionId;
    if (!node)
        return version;

    if (node->is_string()) {
        version.url = tomlString(node);
        return version;
    }

    auto table = node->as_table();
    if (!table)
        return version;

    version.url = tomlString(table->get("url"));
    version.fileName = tomlString(table->get("file_name"));
    version.hashType = tomlString(table->get("hash_type"));
    version.hash = tomlString(table->get("hash"));
    return version;
}

TabDefinition parseToml(const QFileInfo& fileInfo)
{
    TabDefinition tab;
    tab.id = makeTabId(fileInfo);
    tab.sourcePath = fileInfo.absoluteFilePath();
    tab.sourceLastModified = fileInfo.lastModified();
    tab.readmeType = "markdown";

    toml::table root;
#if TOML_EXCEPTIONS
    try {
        root = toml::parse_file(fileInfo.absoluteFilePath().toStdString());
    } catch (const toml::parse_error& err) {
        qWarning(logCustomTabs) << "Failed to parse custom tab toml:" << fileInfo.fileName() << QString(err.what());
        return {};
    }
#else
    auto result = toml::parse_file(fileInfo.absoluteFilePath().toStdString());
    if (!result) {
        qWarning(logCustomTabs) << "Failed to parse custom tab toml:" << fileInfo.fileName() << result.error().description();
        return {};
    }
    root = result.table();
#endif

    tab.name = tomlString(root.get("name"));

    if (auto readme = root["readme"].as_table()) {
        auto type = tomlString(readme->get("type"));
        if (!type.isEmpty())
            tab.readmeType = type;
        tab.readmeContent = tomlString(readme->get("content"));
    }

    if (auto content = root["content"].as_table()) {
        for (auto&& [slugKey, slugNode] : *content) {
            auto slug = QString::fromStdString(std::string(slugKey));
            auto slugTable = slugNode.as_table();
            if (!slugTable)
                continue;

            ContentEntry entry;
            entry.slug = slug;
            entry.name = slug;

            if (auto n = tomlString(slugTable->get("name")); !n.isEmpty())
                entry.name = n;
            entry.description = tomlString(slugTable->get("description"));

            for (auto&& [versionKey, versionNode] : *slugTable) {
                auto versionId = QString::fromStdString(std::string(versionKey));
                if (versionId == "name" || versionId == "description")
                    continue;

                auto version = parseTomlVersion(versionId, &versionNode);
                if (!version.url.isEmpty())
                    entry.versions.append(version);
            }

            if (!entry.versions.isEmpty())
                tab.entries.append(entry);
        }
    }

    if (tab.name.isEmpty())
        tab.name = fileInfo.completeBaseName();

    return tab;
}

bool parseYaml(const QFileInfo& fileInfo, const QByteArray& data, TabDefinition& out)
{
    out = {};
    out.id = makeTabId(fileInfo);
    out.sourcePath = fileInfo.absoluteFilePath();
    out.sourceLastModified = fileInfo.lastModified();
    out.readmeType = "markdown";

    QStringList lines = QString::fromUtf8(data).split('\n');
    auto lineCount = lines.size();

    int i = 0;
    while (i < lineCount) {
        auto raw = lines[i];
        auto trimmed = raw.trimmed();
        if (trimmed.isEmpty() || trimmed.startsWith('#')) {
            i++;
            continue;
        }

        if (trimmed.startsWith("name:")) {
            out.name = trimQuotes(trimmed.mid(5).trimmed());
            i++;
            continue;
        }

        if (trimmed == "readme:") {
            i++;
            while (i < lineCount) {
                auto subRaw = lines[i];
                if (!subRaw.startsWith("  "))
                    break;
                auto sub = subRaw.trimmed();
                if (sub.startsWith("type:")) {
                    auto type = trimQuotes(sub.mid(5).trimmed());
                    if (!type.isEmpty())
                        out.readmeType = type;
                    i++;
                    continue;
                }
                if (sub.startsWith("content:")) {
                    auto contentDecl = sub.mid(8).trimmed();
                    i++;
                    QStringList body;
                    if (contentDecl == "|" || contentDecl == "|-") {
                        while (i < lineCount) {
                            auto bodyLine = lines[i];
                            if (!bodyLine.startsWith("    "))
                                break;
                            body << bodyLine.mid(4);
                            i++;
                        }
                    } else {
                        body << trimQuotes(contentDecl);
                    }
                    out.readmeContent = body.join('\n');
                    continue;
                }
                i++;
            }
            continue;
        }

        if (trimmed == "content:") {
            i++;
            while (i < lineCount) {
                auto slugRaw = lines[i];
                if (!slugRaw.startsWith("  ") || slugRaw.startsWith("    "))
                    break;

                auto slugLine = slugRaw.trimmed();
                if (!slugLine.endsWith(':')) {
                    i++;
                    continue;
                }

                ContentEntry entry;
                entry.slug = trimQuotes(slugLine.left(slugLine.size() - 1).trimmed());
                entry.name = entry.slug;
                i++;

                while (i < lineCount) {
                    auto verRaw = lines[i];
                    if (!verRaw.startsWith("    ") || verRaw.startsWith("      "))
                        break;

                    auto verLine = verRaw.trimmed();
                    if (!verLine.endsWith(':')) {
                        i++;
                        continue;
                    }

                    auto key = trimQuotes(verLine.left(verLine.size() - 1).trimmed());
                    if (key == "name") {
                        i++;
                        continue;
                    }
                    if (key == "description") {
                        i++;
                        continue;
                    }

                    VersionEntry version;
                    version.id = key;
                    i++;

                    while (i < lineCount) {
                        auto urlRaw = lines[i];
                        if (!urlRaw.startsWith("      "))
                            break;
                        auto urlLine = urlRaw.trimmed();
                        if (urlLine.startsWith("url:"))
                            version.url = trimQuotes(urlLine.mid(4).trimmed());
                        else if (urlLine.startsWith("file_name:"))
                            version.fileName = trimQuotes(urlLine.mid(10).trimmed());
                        else if (urlLine.startsWith("hash_type:"))
                            version.hashType = trimQuotes(urlLine.mid(10).trimmed());
                        else if (urlLine.startsWith("hash:"))
                            version.hash = trimQuotes(urlLine.mid(5).trimmed());
                        i++;
                    }

                    if (!version.url.isEmpty())
                        entry.versions.append(version);
                }

                if (!entry.versions.isEmpty())
                    out.entries.append(entry);
            }
            continue;
        }

        i++;
    }

    if (out.name.isEmpty())
        out.name = fileInfo.completeBaseName();

    return !out.entries.isEmpty();
}

bool isValidTab(const TabDefinition& tab)
{
    return !tab.id.isEmpty() && !tab.name.isEmpty() && !tab.entries.isEmpty();
}

}  // namespace

QList<TabDefinition> loadFromDataRoot(const QString& dataRoot)
{
    QList<TabDefinition> tabs;

    QDir root(dataRoot);
    QDir tabsDir(root.filePath("CustomContent/.modtabs"));
    if (!tabsDir.exists())
        return tabs;

    auto files = tabsDir.entryInfoList(QDir::Files | QDir::NoDotAndDotDot, QDir::Name | QDir::IgnoreCase);

    QSet<QString> usedIds;
    for (auto const& fileInfo : files) {
        if (!isSupportedSuffix(fileInfo.suffix()))
            continue;

        QFile file(fileInfo.absoluteFilePath());
        if (!file.open(QIODevice::ReadOnly | QIODevice::Text)) {
            qWarning(logCustomTabs) << "Failed to open custom tab file:" << fileInfo.fileName();
            continue;
        }

        const auto data = file.readAll();
        file.close();

        TabDefinition tab;
        auto suffix = fileInfo.suffix().toLower();
        if (suffix == "json") {
            tab = parseJson(fileInfo, data);
        } else if (suffix == "toml") {
            tab = parseToml(fileInfo);
        } else {
            if (!parseYaml(fileInfo, data, tab)) {
                qWarning(logCustomTabs) << "Failed to parse custom tab yaml:" << fileInfo.fileName();
                continue;
            }
        }

        if (!isValidTab(tab)) {
            qWarning(logCustomTabs) << "Ignoring invalid custom tab file:" << fileInfo.fileName();
            continue;
        }

        QString id = tab.id;
        int counter = 2;
        while (usedIds.contains(id)) {
            id = QString("%1-%2").arg(tab.id).arg(counter);
            counter++;
        }
        tab.id = id;
        usedIds.insert(tab.id);

        tabs.append(tab);
    }

    return tabs;
}

}  // namespace CustomContentTabs
