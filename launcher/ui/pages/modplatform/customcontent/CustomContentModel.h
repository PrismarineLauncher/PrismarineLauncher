// SPDX-FileCopyrightText: 2026
//
// SPDX-License-Identifier: GPL-3.0-only

#pragma once

#include "ui/pages/modplatform/ModModel.h"

namespace ResourceDownload {

class CustomContentModel final : public ModModel {
   public:
    CustomContentModel(BaseInstance& instance, ResourceAPI* api, QString debugName, QString metaEntryBase)
        : ModModel(instance, api, std::move(debugName), std::move(metaEntryBase))
    {}

   protected:
    bool checkVersionFilters(const ModPlatform::IndexedVersion&) override { return true; }
};

}  // namespace ResourceDownload
