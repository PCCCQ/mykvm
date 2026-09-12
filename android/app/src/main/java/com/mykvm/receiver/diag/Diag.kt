package com.mykvm.receiver.diag

import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.core.content.FileProvider
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * The device-side diagnostic log.
 *
 * The Rust core tees its own `log` records into the very same file (see
 * `logger_init` in `android/rust/src/lib.rs`), so one upload contains both
 * halves of the story with a single monotonic clock.
 *
 * Two things matter more than features here:
 *   * the file must never be able to fill the device -- writes are capped and
 *     the log rotates, and
 *   * a failure to log must never take the service down, so every filesystem
 *     call below is best-effort.
 */
object Diag {

    private const val TAG = "MyKvmDiag"
    private const val DIR_NAME = "diagnostics"
    private const val FILE_NAME = "mykvm-android.log"
    private const val ROTATED_NAME = "mykvm-android.1.log"

    /** Rotation threshold; a session rarely reaches a tenth of this. */
    private const val MAX_BYTES = 512L * 1024L

    private val lock = Any()
    private val stamp = SimpleDateFormat("MM-dd HH:mm:ss.SSS", Locale.US)

    @Volatile
    private var logFile: File? = null

    /** Creates the directory and returns the log file. Safe to call repeatedly. */
    fun init(context: Context): File {
        val existing = logFile
        if (existing != null) return existing

        synchronized(lock) {
            logFile?.let { return it }
            val dir = File(context.filesDir, DIR_NAME)
            if (!dir.isDirectory && !dir.mkdirs()) {
                Log.w(TAG, "cannot create $dir")
            }
            val file = File(dir, FILE_NAME)
            logFile = file
            return file
        }
    }

    /** Absolute path handed to the Rust core as `logFile`. */
    fun path(context: Context): String = init(context).absolutePath

    fun file(): File? = logFile

    fun info(message: String) = write("INFO", message, null)

    fun warn(message: String, error: Throwable? = null) = write("WARN", message, error)

    fun error(message: String, error: Throwable? = null) = write("ERROR", message, error)

    /**
     * One line, prefixed with the same shape as the Rust side so the merged file
     * reads as a single stream.
     */
    fun write(level: String, message: String, error: Throwable? = null) {
        // Always keep a copy in logcat: `adb logcat` stays the fastest channel
        // while the tablet is plugged into the PC.
        when (level) {
            "ERROR" -> Log.e(TAG, message, error)
            "WARN" -> Log.w(TAG, message, error)
            else -> Log.i(TAG, message)
        }

        val target = logFile ?: return
        try {
            synchronized(lock) {
                val line = buildString {
                    // `stamp` is a SimpleDateFormat, which is not thread safe;
                    // formatting under the same lock keeps that honest.
                    append(stamp.format(Date()))
                    append(' ')
                    append(level.padEnd(5))
                    append(" mykvm-app: ")
                    append(message)
                    error?.let {
                        append(" -- ")
                        append(it::class.java.simpleName)
                        append(": ")
                        append(it.message)
                    }
                }
                rotateIfNeeded(target)
                target.appendText(line + "\n")
            }
        } catch (failure: Throwable) {
            Log.w(TAG, "cannot append to $target: ${failure.message}")
        }
    }

    /** Everything currently in the log, rotated file first. */
    fun readText(): String {
        val target = logFile ?: return ""
        return try {
            synchronized(lock) {
                val rotated = File(target.parentFile, ROTATED_NAME)
                buildString {
                    if (rotated.isFile) {
                        append(rotated.readText())
                        append('\n')
                    }
                    if (target.isFile) append(target.readText())
                }
            }
        } catch (failure: Throwable) {
            "无法读取日志：${failure.message}"
        }
    }

    fun clear() {
        val target = logFile ?: return
        try {
            synchronized(lock) {
                File(target.parentFile, ROTATED_NAME).delete()
                target.writeText("")
            }
            info("log cleared")
        } catch (failure: Throwable) {
            Log.w(TAG, "cannot clear $target: ${failure.message}")
        }
    }

    /** One-tap hand-off to any other app (mail, chat, Bluetooth, ...). */
    fun shareIntent(context: Context): Intent? {
        val target = logFile ?: init(context)
        if (!target.isFile || target.length() == 0L) {
            info("nothing to share yet")
        }
        return try {
            val uri = FileProvider.getUriForFile(
                context,
                "${context.packageName}.fileprovider",
                target,
            )
            Intent(Intent.ACTION_SEND).apply {
                type = "text/plain"
                putExtra(Intent.EXTRA_STREAM, uri)
                putExtra(Intent.EXTRA_SUBJECT, "MyKVM 安卓端日志")
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
        } catch (failure: Throwable) {
            warn("cannot build the share intent", failure)
            null
        }
    }

    /** Device-side name for the uploaded copy, so a PC can tell tablets apart. */
    fun uploadName(): String {
        val target = logFile
        return target?.name ?: FILE_NAME
    }

    private fun rotateIfNeeded(target: File) {
        if (!target.isFile || target.length() < MAX_BYTES) return
        val rotated = File(target.parentFile, ROTATED_NAME)
        rotated.delete()
        if (target.renameTo(rotated)) return
        // Renaming can fail if the file is open elsewhere; truncating keeps the
        // log bounded either way.
        target.writeText("")
    }
}
