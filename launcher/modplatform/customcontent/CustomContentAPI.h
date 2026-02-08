// SPDX-FileCopyrightText: 2026
//
// SPDX-License-Identifier: GPL-3.0-only

#pragma once

#include <optional>

#include "modplatform/ResourceAPI.h"
#include "modplatform/customcontent/CustomTabConfig.h"

class CustomContentAPI final : public ResourceAPI {
   public:
    CustomContentAPI() = default;
    explicit CustomContentAPI(CustomContentTabs::TabDefinition tab) : m_tab(std::move(tab)) {}

    auto getSortingMethods() const -> QList<SortingMethod> override;

    Task::Ptr searchProjects(SearchArgs&&, Callback<QList<ModPlatform::IndexedPack::Ptr>>&&) const override;

    Task::Ptr getProjects(QStringList addonIds, QByteArray* response) const override;

    auto getSearchURL(SearchArgs const& args) const -> std::optional<QString> override;
    auto getInfoURL(QString const& id) const -> std::optional<QString> override;
    auto getVersionsURL(VersionSearchArgs const& args) const -> std::optional<QString> override;
    auto getDependencyURL(DependencySearchArgs const& args) const -> std::optional<QString> override;

    void loadIndexedPack(ModPlatform::IndexedPack&, QJsonObject&) const override;
    ModPlatform::IndexedVersion loadIndexedPackVersion(QJsonObject& obj, ModPlatform::ResourceType) const override;
    QJsonArray documentToArray(QJsonDocument& obj) const override;
    void loadExtraPackInfo(ModPlatform::IndexedPack&, QJsonObject&) const override;

   private:
    std::optional<CustomContentTabs::TabDefinition> m_tab;
};
