// SPDX-FileCopyrightText: 2026
//
// SPDX-License-Identifier: GPL-3.0-only

#pragma once

#include "ui/pages/modplatform/ModPage.h"

namespace ResourceDownload {

namespace CustomContent {
static inline QString displayName()
{
    return "CustomContent";
}
static inline QIcon icon()
{
    return QIcon::fromTheme("loadermods");
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

    CustomContentModPage(ModDownloadDialog* dialog, BaseInstance& instance);
    ~CustomContentModPage() override = default;

    bool shouldDisplay() const override;

    inline auto displayName() const -> QString override { return CustomContent::displayName(); }
    inline auto icon() const -> QIcon override { return CustomContent::icon(); }
    inline auto id() const -> QString override { return CustomContent::id(); }
    inline auto debugName() const -> QString override { return CustomContent::debugName(); }
    inline auto metaEntryBase() const -> QString override { return CustomContent::metaEntryBase(); }

    inline auto helpPage() const -> QString override { return ""; }

    std::unique_ptr<ModFilterWidget> createFilterWidget() override;
};

}  // namespace ResourceDownload
