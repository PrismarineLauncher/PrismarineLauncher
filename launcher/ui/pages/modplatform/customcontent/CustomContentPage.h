// SPDX-FileCopyrightText: 2026
//
// SPDX-License-Identifier: GPL-3.0-only

#pragma once

#include <optional>

#include "modplatform/customcontent/CustomTabConfig.h"
#include "ui/pages/modplatform/ModPage.h"

namespace ResourceDownload {

namespace CustomContent {
static inline QString displayName()
{
    return "CustomContent";
}
static inline QIcon icon()
{
    return QIcon::fromTheme("customcontent", QIcon::fromTheme("loadermods"));
}
static inline QString id()
{
    return "customcontent";
}
static inline QString debugName()
{
    return "CustomContent";
}
static inline QString metaEntryBase()
{
    return "CustomContentPacks";
}
}  // namespace CustomContent

class CustomContentModPage : public ModPage {
    Q_OBJECT

   public:
    static CustomContentModPage* create(ModDownloadDialog* dialog, BaseInstance& instance)
    {
        return ModPage::create<CustomContentModPage>(dialog, instance);
    }

    static CustomContentModPage* create(ModDownloadDialog* dialog, BaseInstance& instance, CustomContentTabs::TabDefinition tab)
    {
        return ModPage::create<CustomContentModPage>(dialog, instance, std::move(tab));
    }

    CustomContentModPage(ModDownloadDialog* dialog, BaseInstance& instance);
    CustomContentModPage(ModDownloadDialog* dialog, BaseInstance& instance, CustomContentTabs::TabDefinition tab);
    ~CustomContentModPage() override = default;

    bool shouldDisplay() const override;

    inline auto displayName() const -> QString override { return m_displayName; }
    inline auto icon() const -> QIcon override { return m_icon; }
    inline auto id() const -> QString override { return m_id; }
    inline auto debugName() const -> QString override { return m_debugName; }
    inline auto metaEntryBase() const -> QString override { return m_metaEntryBase; }

    inline auto helpPage() const -> QString override { return ""; }

    std::unique_ptr<ModFilterWidget> createFilterWidget() override;

   private:
    void initializePage();
    void setupAddButton();

   private:
    std::optional<CustomContentTabs::TabDefinition> m_tab;

    QString m_displayName = CustomContent::displayName();
    QIcon m_icon = CustomContent::icon();
    QString m_id = CustomContent::id();
    QString m_debugName = CustomContent::debugName();
    QString m_metaEntryBase = CustomContent::metaEntryBase();
};

}  // namespace ResourceDownload
