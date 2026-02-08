// SPDX-FileCopyrightText: 2026
//
// SPDX-License-Identifier: GPL-3.0-only

#include "CustomContentPage.h"

#include "modplatform/customcontent/CustomContentAPI.h"
#include "ui/pages/modplatform/customcontent/CustomContentModel.h"
#include "ui_ResourcePage.h"

#include "Application.h"
#include "FileSystem.h"
#include "minecraft/MinecraftInstance.h"
#include "ui/dialogs/CustomMessageBox.h"
#include "ui/dialogs/ResourceDownloadDialog.h"

#include <QFile>
#include <QFileDialog>
#include <QFileInfo>
#include <QGridLayout>
#include <QHBoxLayout>
#include <QWidget>

namespace ResourceDownload {

CustomContentModPage::CustomContentModPage(ModDownloadDialog* dialog, BaseInstance& instance) : ModPage(dialog, instance)
{
    initializePage();
}

CustomContentModPage::CustomContentModPage(ModDownloadDialog* dialog, BaseInstance& instance, CustomContentTabs::TabDefinition tab)
    : ModPage(dialog, instance), m_tab(std::move(tab))
{
    m_displayName = m_tab->name;
    m_icon = QIcon();
    m_id = m_tab->id;
    m_debugName = QString("CustomContent.%1").arg(m_tab->id);
    m_metaEntryBase = QString("CustomContentPacks.%1").arg(m_tab->id);

    initializePage();
}

void CustomContentModPage::initializePage()
{
    if (m_tab.has_value())
        m_model = new CustomContentModel(m_baseInstance, new CustomContentAPI(*m_tab), debugName(), metaEntryBase());
    else
        m_model = new CustomContentModel(m_baseInstance, new CustomContentAPI(), debugName(), metaEntryBase());

    m_ui->packView->setModel(m_model);

    addSortings();

    connect(m_ui->sortByBox, &QComboBox::currentIndexChanged, this, &CustomContentModPage::triggerSearch);
    connect(m_ui->packView->selectionModel(), &QItemSelectionModel::currentChanged, this, &CustomContentModPage::onSelectionChanged);
    connect(m_ui->versionSelectionBox, &QComboBox::currentIndexChanged, this, &CustomContentModPage::onVersionSelectionChanged);
    connect(m_ui->resourceSelectionButton, &QPushButton::clicked, this, &CustomContentModPage::onResourceSelected);

    if (!m_tab.has_value())
        setupAddButton();

    m_ui->packDescription->setMetaEntry(metaEntryBase());
}

void CustomContentModPage::setupAddButton()
{
    auto addButton = new QPushButton(tr("&Add File"), this);
    addButton->setToolTip(tr("Add a local .jar file to Custom Content"));

    if (auto grid = qobject_cast<QGridLayout*>(m_ui->gridLayout_4)) {
        grid->removeWidget(m_ui->sortByBox);

        auto sortContainer = new QWidget(this);
        auto sortLayout = new QHBoxLayout(sortContainer);
        sortLayout->setContentsMargins(0, 0, 0, 0);
        sortLayout->addWidget(m_ui->sortByBox);
        sortLayout->addWidget(addButton);

        grid->addWidget(sortContainer, 0, 0);
    }

    connect(addButton, &QPushButton::clicked, this, [this] {
        auto file_path = QFileDialog::getOpenFileName(this, tr("Add mod file"), QString(), tr("Java mods (*.jar)"));
        if (file_path.isEmpty())
            return;

        QDir root(APPLICATION->dataRoot());
        QDir custom_dir(root.filePath("CustomContent"));
        if (!FS::ensureFolderPathExists(custom_dir.absolutePath())) {
            CustomMessageBox::selectable(this, tr("Error"),
                                         tr("Failed to create Custom Content folder:\n%1").arg(custom_dir.absolutePath()),
                                         QMessageBox::Critical)
                ->exec();
            return;
        }

        QFileInfo src_info(file_path);
        auto dest_path = custom_dir.absoluteFilePath(src_info.fileName());
        if (QFileInfo::exists(dest_path)) {
            auto reply = CustomMessageBox::selectable(this, tr("File exists"),
                                                      tr("'%1' already exists in Custom Content.\nReplace it?")
                                                          .arg(src_info.fileName()),
                                                      QMessageBox::Question, QMessageBox::Yes | QMessageBox::No, QMessageBox::No)
                             ->exec();
            if (reply != QMessageBox::Yes)
                return;
            QFile::remove(dest_path);
        }

        if (!QFile::copy(file_path, dest_path)) {
            CustomMessageBox::selectable(this, tr("Error"),
                                         tr("Failed to copy '%1' to Custom Content.").arg(src_info.fileName()),
                                         QMessageBox::Critical)
                ->exec();
            return;
        }

        triggerSearch();
    });
}

auto CustomContentModPage::shouldDisplay() const -> bool
{
    return true;
}

std::unique_ptr<ModFilterWidget> CustomContentModPage::createFilterWidget()
{
    return ModFilterWidget::create(&static_cast<MinecraftInstance&>(m_baseInstance), false);
}

}  // namespace ResourceDownload
