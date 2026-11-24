package xyz.ponbac.sparrow.shield;

import android.app.Activity;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

public final class FixtureReceiver extends BroadcastReceiver {
    static {
        System.loadLibrary("sparrow_shield");
    }

    @Override
    public void onReceive(Context context, Intent intent) {
        String action = intent.getAction();
        int command;
        if ("xyz.ponbac.sparrow.shield.SEED_FIXTURE".equals(action)) {
            command = 1;
        } else if ("xyz.ponbac.sparrow.shield.CHECK_HTTPS".equals(action)) {
            command = 2;
        } else {
            return;
        }
        if (nativeCommand(command)) {
            setResultCode(Activity.RESULT_OK);
        }
    }

    private static native boolean nativeCommand(int command);
}
