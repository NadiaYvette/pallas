{-# LANGUAGE NumericUnderscores #-}
{-# LANGUAGE RecordWildCards #-}
{-# LANGUAGE ScopedTypeVariables #-}
{-# OPTIONS_GHC -Wno-unused-matches #-}

-- | Test that Pallas trace-compat (Rust tracer) can receive traces from
--   the Haskell trace-forward library, acting as a drop-in replacement
--   for cardano-tracer.

import           Cardano.Logging
import qualified Cardano.Logging.Types as Net
import           Cardano.Tracer.Test.ForwardingStressTest.Script
import           Cardano.Tracer.Test.ForwardingStressTest.Types
import           Cardano.Tracer.Test.Utils
import           Ouroboros.Network.Magic (NetworkMagic (..))
import           Ouroboros.Network.NodeToClient (withIOManager)

import           Control.Concurrent (threadDelay)
import           Control.Exception
import           Control.Monad.Extra
import           Data.Functor ((<&>))
import           Data.IORef (IORef, newIORef, readIORef, writeIORef)
import qualified Data.List as L
import           Data.Maybe (fromMaybe)
import           Data.Monoid
import           Data.Vector (Vector)
import qualified Data.Vector as Vector
import qualified System.Directory as Sys
import           System.Environment (lookupEnv, setEnv, unsetEnv)
import qualified System.IO as Sys
import           System.PosixCompat.Files (fileExist)
import qualified System.Process as Sys

import           Test.Tasty
import           Test.Tasty.QuickCheck

import           Trace.Forward.Forwarding (initForwarding)
import           Trace.Forward.Utils.TraceObject (writeToSink)

main :: IO ()
main = do
    setEnv "TASTY_NUM_THREADS" "1"
    mbWorkdir <- lookupEnv "WORKDIR"

    ts' <- getTestSetup
             TestSetup
             { tsTime         = Last $ Just 10.0
             , tsThreads      = Last $ Just 5
             , tsMessages     = Last   Nothing
             , tsSockInternal = Last $ Just "tracer.sock"
             , tsSockExternal = Last $ Just "tracer.sock"
             , tsNetworkMagic = Last $ Just $ NetworkMagic 42
             , tsWorkDir      = Last $ Just $ fromMaybe "/tmp/testTracerCompat" mbWorkdir
             }

    tracerRoot <- Sys.canonicalizePath $ unI (tsWorkDir ts')

    putStrLn . mconcat $ [ "tsWorkDir ts: ", tracerRoot ]
    whenM (fileExist                         tracerRoot) do
      Sys.removeDirectoryRecursive           tracerRoot
    Sys.createDirectoryIfMissing True       (tracerRoot <> "/logs")
    Sys.setCurrentDirectory                  tracerRoot

    let ts = ts' { tsWorkDir      = Identity tracerRoot
                 }
    putStrLn $ "Test setup:  " <> show ts

    msgCountersRef <- newIORef []
    msgsRef        <- newIORef Vector.empty
    tracerRef      <- newIORef Nothing
    let tracerGetter = getCompatTracerState ts tracerRef
    defaultMain (allTests ts msgCountersRef msgsRef (tracerGetter <&> snd))
        `catch` (\ (e :: SomeException) -> do
            unsetEnv "TASTY_NUM_THREADS"
            trState <- readIORef tracerRef
            case trState of
              Nothing -> pure ()
              Just (tracerHdl, _) ->
                Sys.cleanupProcess (Nothing, Nothing, Nothing, tracerHdl)
            throwIO e)

allTests ::
     TestSetup Identity
  -> IORef [Int]
  -> IORef (Vector Message)
  -> IO (Trace IO Message)
  -> TestTree
allTests ts msgCountersRef msgsRef externalTracerGetter =
    testGroup "Tests"
    [ localOption (QuickCheckTests 3) $ testGroup "trace-compat"
        [ testProperty "multi-threaded forwarder stress test via trace-compat" $
            runScriptForwarding ts msgCountersRef msgsRef externalTracerGetter
        ]
    ]

-- | Start trace-compat (Rust tracer) directly on tracer.sock.
--   No proxy, no cardano-tracer — the Rust binary handles everything.
getCompatTracerState ::
     TestSetup Identity
  -> IORef (Maybe (Sys.ProcessHandle, Trace IO Message))
  -> IO (Sys.ProcessHandle, Trace IO Message)
getCompatTracerState TestSetup{..} ref = do
  state <- readIORef ref
  case state of
    Just st -> pure st
    Nothing -> do
      stdTr <- standardTracer
      (procHdl, fwdTr) <- setupFwdTracer
      tr <- mkCardanoTracer
              stdTr fwdTr Nothing
              ["Test"]
      let st = (procHdl, tr)
      writeIORef ref $ Just st
      pure st
 where
   setupFwdTracer :: IO (Sys.ProcessHandle, Trace IO FormattedMessage)
   setupFwdTracer = do
     -- Write config for trace-compat listening on tracer.sock
     Sys.writeFile "config.yaml" . L.unlines $
       [ "networkMagic: " <> show (unNetworkMagic $ unI tsNetworkMagic)
       , "network:"
       , "  tag: AcceptAt"
       , "  contents: \""<> unI tsSockExternal <>"\""
       , "logging:"
       , "- logRoot: \"logs\""
       , "  logMode: FileMode"
       , "  logFormat: ForMachine"
       ]
     -- Start trace-compat directly on tracer.sock
     binDir <- fromMaybe "../../../target/release" <$> lookupEnv "PALLAS_BIN_DIR"
     compatHdl <- Sys.spawnProcess (binDir <> "/trace-compat")
       [ "--config" ,    "config.yaml"
       ]
     threadDelay 2_000_000
     res <- Sys.getProcessExitCode compatHdl
     case res of
       Nothing   -> putStrLn "trace-compat started.."
       Just code ->
         error $ "trace-compat failed to start with code " <> show code
     -- Initialize forwarding connecting directly to tracer.sock (trace-compat)
     (forwardSink, _dpStore) <- withIOManager \iomgr -> do
       let tracerSocketMode = Just (Net.LocalPipe (unI tsSockExternal), Initiator)
           forwardingConf = fromMaybe defaultForwarder (tcForwarder simpleTestConfig)
       initForwarding iomgr forwardingConf (unI tsNetworkMagic) Nothing tracerSocketMode
     pure (compatHdl, forwardTracer (writeToSink forwardSink))
