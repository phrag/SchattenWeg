package de.schattenweg.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.lifecycle.ViewModelProvider
import androidx.compose.ui.graphics.Color

class MainActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val viewModel = ViewModelProvider(this)[RouteViewModel::class.java]
        // Provision the bundled Berlin snapshot into app-private storage and
        // hand the Rust core its path. Idempotent; cheap after first launch.
        viewModel.initialise(MapAssets.ensure(this).routingPbf?.absolutePath)

        setContent {
            MaterialTheme(colorScheme = SchattenwegColors) {
                MapScreen(viewModel)
            }
        }
    }
}

private val SchattenwegColors = darkColorScheme(
    primary = Color(0xFF7FD4A2),
    background = Color(0xFF10141A),
    surface = Color(0xFF161B22),
    onBackground = Color(0xFFF2F4F8),
    onSurface = Color(0xFFF2F4F8),
)
