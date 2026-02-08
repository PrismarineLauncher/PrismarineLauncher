// SPDX-FileCopyrightText: 2026
//
// SPDX-License-Identifier: GPL-3.0-only

#pragma once

#include <QDateTime>
#include <QList>
#include <QString>

namespace CustomContentTabs {

struct VersionEntry {
    QString id;
    QString url;
    QString fileName;
    QString hashType;
    QString hash;
};

struct ContentEntry {
    QString slug;
    QString name;
    QString description;
    QList<VersionEntry> versions;
};

struct TabDefinition {
    QString id;
    QString name;
    QString sourcePath;
    QString readmeType;
    QString readmeContent;
    QDateTime sourceLastModified;
    QList<ContentEntry> entries;
};

QList<TabDefinition> loadFromDataRoot(const QString& dataRoot);

}  // namespace CustomContentTabs
