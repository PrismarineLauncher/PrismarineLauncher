#include "ParseUtils.h"
#include <QDateTime>
#include <QDebug>
#include <QString>
#include <QTimeZone>
#include <cstdlib>

#ifdef LAUNCHER_ENABLE_RUST_CORE
#include "rust/RustCoreFFI.h"
#endif

QDateTime timeFromS3Time(QString str)
{
#ifdef LAUNCHER_ENABLE_RUST_CORE
    PrismarineTimestampResult parsed{};
    auto utf8 = str.toUtf8();
    if (prismarine_parse_s3_time(utf8.constData(), &parsed) == 0 && parsed.is_valid != 0) {
        auto tz = QTimeZone::fromSecondsAheadOfUtc(parsed.offset_seconds);
        auto fromRust = QDateTime::fromMSecsSinceEpoch(parsed.unix_ms_utc, tz);
        if (fromRust.isValid()) {
            return fromRust;
        }
    }
#endif
    return QDateTime::fromString(str, Qt::ISODate);
}

QString timeToS3Time(QDateTime time)
{
#ifdef LAUNCHER_ENABLE_RUST_CORE
    QByteArray outBuffer(64, '\0');
    if (prismarine_format_s3_time(time.toMSecsSinceEpoch(), time.offsetFromUtc(), outBuffer.data(), outBuffer.size()) == 0) {
        return QString::fromUtf8(outBuffer.constData());
    }
#endif
    // this all because Qt can't format timestamps right.
    int offsetRaw = time.offsetFromUtc();
    bool negative = offsetRaw < 0;
    int offsetAbs = std::abs(offsetRaw);

    int offsetSeconds = offsetAbs % 60;
    offsetAbs -= offsetSeconds;

    int offsetMinutes = offsetAbs % 3600;
    offsetAbs -= offsetMinutes;
    offsetMinutes /= 60;

    int offsetHours = offsetAbs / 3600;

    QString raw = time.toString("yyyy-MM-ddTHH:mm:ss");
    raw += (negative ? QChar('-') : QChar('+'));
    raw += QString("%1").arg(offsetHours, 2, 10, QChar('0'));
    raw += ":";
    raw += QString("%1").arg(offsetMinutes, 2, 10, QChar('0'));
    return raw;
}
