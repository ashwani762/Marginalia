package app.marginalia

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import java.net.Inet4Address

/** A home computer found on the network, as the core wants to hear about it. */
data class SeenNode(val name: String, val id: String, val port: Int, val addrs: List<String>)

/**
 * Finds `marginalia-node` computers on the local network and tells [report] about each one.
 * It only runs while the app is on screen, and does nothing but listen for the computers' announcements.
 * Every NsdManager callback is hopped onto the main thread so this class needs no locks.
 */
class NodeDiscovery(context: Context, private val report: (SeenNode) -> Unit) {
    private val nsd = context.applicationContext.getSystemService(Context.NSD_SERVICE) as? NsdManager
    private val main = Handler(Looper.getMainLooper())

    private var running = false
    private var discovery: NsdManager.DiscoveryListener? = null

    /** What we last learned about each service, keyed by service name. */
    private val seen = HashMap<String, SeenNode>()

    // Android 14 and later: one live callback per service.
    private val infoCallbacks = HashMap<String, Any>()

    // Before Android 14 only one resolve may run at a time, so they wait in line.
    private val queue = ArrayDeque<NsdServiceInfo>()
    private var resolving = false

    private val refresh = object : Runnable {
        override fun run() {
            if (!running) return
            // Tell the core again now and then so a computer that stays put does not look gone.
            seen.values.forEach(report)
            main.postDelayed(this, REPEAT_MS)
        }
    }

    fun start() {
        main.post { startNow() }
    }

    fun stop() {
        main.post { stopNow() }
    }

    private fun startNow() {
        val mgr = nsd ?: return
        if (running) return
        running = true
        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) { Log.i(LOG, "discovery started for $serviceType") }
            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) { Log.w(LOG, "discovery failed to start: $errorCode"); main.post { if (discovery === this) { discovery = null; running = false } } }
            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
            override fun onDiscoveryStopped(serviceType: String) {}
            override fun onServiceFound(info: NsdServiceInfo) { Log.i(LOG, "found ${info.serviceName} ${info.serviceType}"); main.post { if (running && discovery === this) found(info) } }
            override fun onServiceLost(info: NsdServiceInfo) { main.post { if (discovery === this) lost(info) } }
        }
        discovery = listener
        try {
            // NsdManager reports types with a trailing dot but accepts them without, so the match ignores it.
            mgr.discoverServices(TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
        } catch (_: Exception) {
            discovery = null
            running = false
            return
        }
        main.postDelayed(refresh, REPEAT_MS)
    }

    private fun stopNow() {
        if (!running) return
        running = false
        main.removeCallbacks(refresh)
        val mgr = nsd
        val l = discovery
        discovery = null
        if (mgr != null && l != null) runCatching { mgr.stopServiceDiscovery(l) }
        if (mgr != null && Build.VERSION.SDK_INT >= 34) {
            infoCallbacks.values.forEach { cb -> runCatching { mgr.unregisterServiceInfoCallback(cb as NsdManager.ServiceInfoCallback) } }
        }
        infoCallbacks.clear()
        queue.clear()
        resolving = false
        seen.clear()
    }

    private fun isOurs(info: NsdServiceInfo): Boolean =
        info.serviceType.orEmpty().trimEnd('.').equals(TYPE, ignoreCase = true)

    private fun found(info: NsdServiceInfo) {
        if (!isOurs(info)) return
        if (Build.VERSION.SDK_INT >= 34) watch(info) else enqueue(info)
    }

    private fun lost(info: NsdServiceInfo) {
        val name = info.serviceName ?: return
        seen.remove(name)
        queue.removeAll { it.serviceName == name }
        if (Build.VERSION.SDK_INT >= 34) {
            val cb = infoCallbacks.remove(name) as? NsdManager.ServiceInfoCallback ?: return
            runCatching { nsd?.unregisterServiceInfoCallback(cb) }
        }
    }

    private fun accept(info: NsdServiceInfo) {
        val addrs = addressesOf(info)
        if (addrs.isEmpty() || info.port <= 0) return
        val attrs = info.attributes.orEmpty()
        fun txt(key: String): String = attrs[key]?.toString(Charsets.UTF_8).orEmpty().trim()
        val node = SeenNode(
            name = txt("name").ifBlank { info.serviceName.orEmpty() },
            id = txt("id").lowercase(),
            port = info.port,
            addrs = addrs,
        )
        Log.i(LOG, "resolved ${node.name} ${node.addrs} port ${node.port} id ${node.id}")
        seen[info.serviceName.orEmpty()] = node
        report(node)
    }

    @Suppress("DEPRECATION")
    private fun addressesOf(info: NsdServiceInfo): List<String> {
        val all = if (Build.VERSION.SDK_INT >= 34) info.hostAddresses else listOfNotNull(info.host)
        return all.filterIsInstance<Inet4Address>().mapNotNull { it.hostAddress }.distinct()
    }

    // ---- Android 14 and later

    private fun watch(info: NsdServiceInfo) {
        val mgr = nsd ?: return
        if (Build.VERSION.SDK_INT < 34) return
        val name = info.serviceName ?: return
        if (infoCallbacks.containsKey(name)) return
        val cb = object : NsdManager.ServiceInfoCallback {
            override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) { main.post { infoCallbacks.remove(name) } }
            override fun onServiceUpdated(serviceInfo: NsdServiceInfo) { main.post { if (running && infoCallbacks[name] === this) accept(serviceInfo) } }
            override fun onServiceLost() { main.post { if (infoCallbacks[name] === this) { infoCallbacks.remove(name); seen.remove(name) } } }
            override fun onServiceInfoCallbackUnregistered() {}
        }
        infoCallbacks[name] = cb
        try {
            mgr.registerServiceInfoCallback(info, { r -> r.run() }, cb)
        } catch (_: Exception) {
            infoCallbacks.remove(name)
        }
    }

    // ---- Before Android 14

    private fun enqueue(info: NsdServiceInfo) {
        if (queue.none { it.serviceName == info.serviceName }) queue.addLast(info)
        next()
    }

    @Suppress("DEPRECATION")
    private fun next() {
        val mgr = nsd ?: return
        if (resolving || !running) return
        val info = queue.removeFirstOrNull() ?: return
        resolving = true
        try {
            mgr.resolveService(info, object : NsdManager.ResolveListener {
                override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                    main.post {
                        resolving = false
                        // Another resolve was still going. Try this one again a little later.
                        if (errorCode == NsdManager.FAILURE_ALREADY_ACTIVE && running) {
                            main.postDelayed({ if (running) enqueue(info) }, RETRY_MS)
                        }
                        next()
                    }
                }

                override fun onServiceResolved(serviceInfo: NsdServiceInfo) {
                    main.post {
                        resolving = false
                        if (running) accept(serviceInfo)
                        next()
                    }
                }
            })
        } catch (_: Exception) {
            resolving = false
        }
    }

    companion object {
        private const val LOG = "NodeDiscovery"
        private const val TYPE = "_marginalia._udp"
        private const val REPEAT_MS = 30_000L
        private const val RETRY_MS = 500L
    }
}
