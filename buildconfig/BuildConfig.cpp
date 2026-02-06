// SPDX-License-Identifier: GPL-3.0-only
/*
 *  Amethyst Launcher - Minecraft Launcher
 *  Copyright (C) 2022 Sefa Eyeoglu <contact@scrumplex.net>
 *
 *  This program is free software: you can redistribute it and/or modify
 *  it under the terms of the GNU General Public License as published by
 *  the Free Software Foundation, version 3.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *  GNU General Public License for more details.
 *
 *  You should have received a copy of the GNU General Public License
 *  along with this program.  If not, see <https://www.gnu.org/licenses/>.
 *
 * This file incorporates work covered by the following copyright and
 * permission notice:
 *
 *      Copyright 2013-2021 MultiMC Contributors
 *
 *      Licensed under the Apache License, Version 2.0 (the "License");
 *      you may not use this file except in compliance with the License.
 *      You may obtain a copy of the License at
 *
 *          http://www.apache.org/licenses/LICENSE-2.0
 *
 *      Unless required by applicable law or agreed to in writing, software
 *      distributed under the License is distributed on an "AS IS" BASIS,
 *      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *      See the License for the specific language governing permissions and
 *      limitations under the License.
 */

#include <QObject>
#include "BuildConfig.h"

const Config BuildConfig;

Config::Config()
{
    // Name and copyright
    LAUNCHER_NAME = "AmethystLauncher";
    LAUNCHER_APP_BINARY_NAME = "prismlauncher";
    LAUNCHER_DISPLAYNAME = "Amethyst Launcher";
    LAUNCHER_COPYRIGHT = "© 2022-2026 Amethyst Launcher Contributors\n© 2021-2022 PolyMC Contributors\n© 2012-2021 MultiMC Contributors";
    LAUNCHER_DOMAIN = "prismlauncher.org";
    LAUNCHER_CONFIGFILE = "prismlauncher.cfg";
    LAUNCHER_GIT = "https://github.com/AmethystLauncher/AmethystLauncher";
    LAUNCHER_APPID = "org.prismlauncher.AmethystLauncher";
    LAUNCHER_SVGFILENAME = "org.prismlauncher.AmethystLauncher.svg";

    USER_AGENT = "AmethystLauncher/11.0.0";

    // Version information
    VERSION_MAJOR = 11;
    VERSION_MINOR = 0;
    VERSION_PATCH = 0;

    BUILD_PLATFORM = "unknown";
    BUILD_ARTIFACT = "";
    BUILD_DATE = "2026-02-06";
    UPDATER_GITHUB_REPO = "https://github.com/AmethystLauncher/AmethystLauncher";

    COMPILER_NAME = "GNU";
    COMPILER_VERSION = "15.2.1";

    COMPILER_TARGET_SYSTEM = "Linux";
    COMPILER_TARGET_SYSTEM_VERSION = "6.18.7-arch1-1";
    COMPILER_TARGET_SYSTEM_PROCESSOR = "x86_64";

    MAC_SPARKLE_PUB_KEY = "";
    MAC_SPARKLE_APPCAST_URL = "";

    if (!MAC_SPARKLE_PUB_KEY.isEmpty() && !MAC_SPARKLE_APPCAST_URL.isEmpty()) {
        UPDATER_ENABLED = true;
    } else if (!UPDATER_GITHUB_REPO.isEmpty() && !BUILD_ARTIFACT.isEmpty()) {
        UPDATER_ENABLED = true;
    }

#define Launcher_ENABLE_JAVA_DOWNLOADER 0
    JAVA_DOWNLOADER_ENABLED = Launcher_ENABLE_JAVA_DOWNLOADER;

    GIT_COMMIT = "9171e2b2e171acc34a8bff876c2ed697987c50f7";
    GIT_TAG = "-128-NOTFOUND";
    GIT_REFSPEC = "refs/heads/develop";

    // Assume that builds outside of Git repos are "stable"
    if (GIT_REFSPEC == QStringLiteral("GITDIR-NOTFOUND") || GIT_TAG == QStringLiteral("GITDIR-NOTFOUND") ||
        GIT_REFSPEC == QStringLiteral("") || GIT_TAG == QStringLiteral("GIT-NOTFOUND")) {
        GIT_REFSPEC = "refs/heads/stable";
        GIT_TAG = versionString();
        GIT_COMMIT = "";
    }

    if (GIT_REFSPEC.startsWith("refs/heads/")) {
        VERSION_CHANNEL = GIT_REFSPEC;
        VERSION_CHANNEL.remove("refs/heads/");
    } else if (!GIT_COMMIT.isEmpty()) {
        VERSION_CHANNEL = GIT_COMMIT.mid(0, 8);
    } else {
        VERSION_CHANNEL = "unknown";
    }

    NEWS_RSS_URL = "https://prismlauncher.org/feed/feed.xml";
    NEWS_OPEN_URL = "https://prismlauncher.org/news";
    HELP_URL = "https://prismlauncher.org/wiki/help-pages/%1";
    LOGIN_CALLBACK_URL = "https://prismlauncher.org/successful-login";
    IMGUR_CLIENT_ID = "5b97b0713fba4a3";
    MSA_CLIENT_ID = "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb";
    FLAME_API_KEY = "$2a$10$wuAJuNZuted3NORVmpgUC.m8sI.pv1tOPKZyBgLFGjxFp/br0lZCC";
    META_URL = "https://meta.prismlauncher.org/v1/";
    LEGACY_FMLLIBS_BASE_URL = "https://files.prismlauncher.org/fmllibs/";

    GLFW_LIBRARY_NAME = "libglfw.so";
    OPENAL_LIBRARY_NAME = "libopenal.so";

    BUG_TRACKER_URL = "https://github.com/AmethystLauncher/AmethystLauncher/issues";
    TRANSLATIONS_URL = "https://hosted.weblate.org/projects/prismlauncher/launcher/";
    TRANSLATION_FILES_URL = "https://i18n.prismlauncher.org/";
    MATRIX_URL = "https://prismlauncher.org/matrix";
    DISCORD_URL = "https://prismlauncher.org/discord";
    SUBREDDIT_URL = "https://prismlauncher.org/reddit";
}

QString Config::versionString() const
{
    return QString("%1.%2.%3").arg(VERSION_MAJOR).arg(VERSION_MINOR).arg(VERSION_PATCH);
}

QString Config::printableVersionString() const
{
    QString vstr = versionString();

    // If the build is not a main release, append the channel
    if (VERSION_CHANNEL != "stable" && GIT_TAG != vstr) {
        vstr += "-" + VERSION_CHANNEL;
    }
    return vstr;
}

QString Config::compilerID() const
{
    if (COMPILER_VERSION.isEmpty())
        return COMPILER_NAME;
    return QStringLiteral("%1 - %2").arg(COMPILER_NAME).arg(COMPILER_VERSION);
}

QString Config::systemID() const
{
    return QStringLiteral("%1 %2 %3").arg(COMPILER_TARGET_SYSTEM, COMPILER_TARGET_SYSTEM_VERSION, COMPILER_TARGET_SYSTEM_PROCESSOR);
}
