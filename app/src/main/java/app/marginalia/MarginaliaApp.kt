package app.marginalia

import android.app.Application

/**
 * The app's brain lives here, not in a screen. It starts with the process and stays for as long as
 * the process does, which a foreground service extends past the last window: notes keep being
 * written and the "notes ready" notification still fires after the app is swiped away.
 */
class MarginaliaApp : Application() {
    lateinit var vm: AppVm
        private set

    override fun onCreate() {
        super.onCreate()
        vm = AppVm(this)
    }
}
